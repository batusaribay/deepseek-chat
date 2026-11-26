//! Turns the page's Markdown into plain terminal text.
//!
//! No ANSI, no colour, no box drawing: the output is ordinary text that
//! scrolls back, pipes, and copies cleanly. Markdown markers are stripped so
//! `**bold**` prints as `bold` and fenced code prints as itself.
//!
//! Answers arrive in pieces and the page re-renders while it streams, so the
//! renderer writes only complete lines and re-reads the unfinished tail from
//! the current text instead of appending deltas.

use std::io::{self, Write};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Block {
    Fence,
    Heading,
    Bullet,
    Quote,
    Rule,
    Plain,
}

pub struct Stream<W: Write> {
    out: W,
    /// Bytes of the current answer already written.
    emitted: usize,
    /// Everything after the last newline, replaced on every update.
    pending: String,
    in_fence: bool,
    line_start: bool,
    /// Characters held back at the start of a line, to recognise blocks.
    head: String,
    /// Collecting the info string of an opening fence.
    opening: Option<String>,
    /// Swallow the rest of a line we already rendered.
    skip_line: bool,
    /// Run of `*` waiting to be dropped (emphasis markers).
    stars: usize,
}

impl<W: Write> Stream<W> {
    pub fn new(out: W) -> Self {
        Self {
            out,
            emitted: 0,
            pending: String::new(),
            in_fence: false,
            line_start: true,
            head: String::new(),
            opening: None,
            skip_line: false,
            stars: 0,
        }
    }

    /// How much of the answer has already been written.
    pub fn consumed(&self) -> usize {
        self.emitted
    }

    /// Take the writer back once the answer is complete.
    pub fn into_writer(self) -> W {
        self.out
    }

    /// Feed the whole answer so far; only complete lines are written.
    pub fn feed(&mut self, full: &str) -> io::Result<()> {
        let end = full.rfind('\n').map(|i| i + 1).unwrap_or(0);

        if end > self.emitted && self.emitted <= full.len() && full.is_char_boundary(self.emitted) {
            let chunk = full[self.emitted..end].to_string();
            self.emitted = end;
            self.emit(&chunk)?;
        }

        self.pending = full[end..].to_string();
        Ok(())
    }

    fn emit(&mut self, text: &str) -> io::Result<()> {
        let chars: Vec<char> = text.chars().collect();
        for ch in chars {
            self.put(ch)?;
        }
        self.out.flush()
    }

    /// Flush anything held back.
    pub fn finish(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let rest = std::mem::take(&mut self.pending);
            self.emitted += rest.len();
            self.emit(&rest)?;
        }
        if !self.head.is_empty() {
            let head = std::mem::take(&mut self.head);
            match classify(&head) {
                Some(Block::Fence) => {}
                Some(Block::Heading) => {
                    write_visible(&mut self.out, head.trim_start_matches('#').trim_start())?
                }
                _ => write_visible(&mut self.out, &head)?,
            }
        }
        if self.stars > 0 {
            self.stars = 0;
        }
        self.out.flush()
    }

    fn put(&mut self, ch: char) -> io::Result<()> {
        if let Some(lang) = self.opening.as_mut() {
            if ch == '\n' {
                self.opening = None;
                self.out.write_all(b"\n")?;
                self.line_start = true;
            } else {
                lang.push(ch);
            }
            return Ok(());
        }

        if self.skip_line {
            if ch == '\n' {
                self.skip_line = false;
                self.line_start = true;
            }
            return Ok(());
        }

        if self.in_fence {
            if self.line_start {
                self.head.push(ch);
                if self.head.len() < 3 && "```".starts_with(self.head.as_str()) {
                    return Ok(());
                }
                if self.head == "```" {
                    self.in_fence = false;
                    self.head.clear();
                    self.out.write_all(b"\n")?;
                    self.skip_line = true;
                    return Ok(());
                }
                let head = std::mem::take(&mut self.head);
                write_visible(&mut self.out, &head)?;
                self.line_start = false;
                return Ok(());
            }
            if ch == '\n' {
                self.out.write_all(b"\n")?;
                self.line_start = true;
                return Ok(());
            }
            let mut buf = [0u8; 4];
            return match visible(ch) {
                Some(ch) => self.out.write_all(ch.encode_utf8(&mut buf).as_bytes()),
                None => Ok(()),
            };
        }

        if self.line_start {
            self.head.push(ch);
            if let Some(block) = classify(&self.head) {
                let head = std::mem::take(&mut self.head);
                self.line_start = false;
                return self.begin_block(block, &head);
            }
            return Ok(());
        }

        self.inline(ch)
    }

    fn begin_block(&mut self, block: Block, head: &str) -> io::Result<()> {
        match block {
            Block::Fence => {
                self.in_fence = true;
                self.opening = Some(head.trim_start_matches('`').to_string());
                Ok(())
            }
            Block::Heading => {
                write_visible(&mut self.out, head.trim_start_matches('#').trim_start())
            }
            Block::Bullet => self.out.write_all(b"- "),
            Block::Quote => self.out.write_all(b"> "),
            Block::Rule => {
                self.out.write_all(b"----\n")?;
                self.skip_line = true;
                Ok(())
            }
            Block::Plain => {
                let chars: Vec<char> = head.chars().collect();
                for ch in chars {
                    self.inline(ch)?;
                }
                Ok(())
            }
        }
    }

    fn inline(&mut self, ch: char) -> io::Result<()> {
        // The answer comes from a web page, so it must not be able to drive
        // the terminal with control sequences.
        if visible(ch).is_none() {
            return Ok(());
        }
        match ch {
            '\n' => {
                self.stars = 0;
                self.out.write_all(b"\n")?;
                self.line_start = true;
                return Ok(());
            }
            '*' | '_' | '`' => {
                // Emphasis and code markers are dropped, not printed.
                self.stars += 1;
                return Ok(());
            }
            _ => {}
        }

        self.stars = 0;
        let mut buf = [0u8; 4];
        self.out.write_all(ch.encode_utf8(&mut buf).as_bytes())
    }
}

/// Control characters would let a page rewrite the terminal (or the clipboard
/// through OSC sequences), so only newlines, tabs and printable text survive.
fn visible(ch: char) -> Option<char> {
    (!ch.is_control() || ch == '\n' || ch == '\t').then_some(ch)
}

fn write_visible<W: Write>(out: &mut W, text: &str) -> io::Result<()> {
    let mut buf = [0u8; 4];
    for ch in text.chars() {
        if let Some(ch) = visible(ch) {
            out.write_all(ch.encode_utf8(&mut buf).as_bytes())?;
        }
    }
    Ok(())
}

/// Is the held-back line prefix recognisable yet?
fn classify(head: &str) -> Option<Block> {
    if head.is_empty() {
        return None;
    }
    if "```".starts_with(head) && head.len() < 3 {
        return None;
    }
    if head.starts_with("```") {
        return Some(Block::Fence);
    }
    if head == "- " || head == "* " || head == "+ " {
        return Some(Block::Bullet);
    }
    if head == "> " {
        return Some(Block::Quote);
    }
    if head == "-" || head == "*" || head == "+" || head == ">" {
        return None;
    }
    if head == "---" || head == "***" || head == "___" {
        return Some(Block::Rule);
    }
    if head == "--" || head == "**" || head == "__" {
        return None;
    }

    if head.starts_with('#') {
        let hashes = head.chars().take_while(|c| *c == '#').count();
        if hashes > 6 {
            return Some(Block::Plain);
        }
        if head.len() <= hashes {
            return None;
        }
        return Some(if head[hashes..].starts_with(' ') {
            Block::Heading
        } else {
            Block::Plain
        });
    }

    Some(Block::Plain)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(text: &str) -> String {
        let mut out = Vec::new();
        {
            let mut s = Stream::new(&mut out);
            s.feed(text).unwrap();
            s.finish().unwrap();
        }
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn output_has_no_ansi() {
        let out = render("# Head\n\nSome **bold**, *italic*, `code`, and a [link](https://x).\n");
        assert!(!out.contains('\x1b'), "got {out:?}");
        assert_eq!(
            out,
            "Head\n\nSome bold, italic, code, and a [link](https://x).\n"
        );
    }

    #[test]
    fn strips_control_sequences_from_answers() {
        let out = render("safe \x1b[31mred\x1b[0m text\n");
        assert!(!out.contains('\x1b'), "got {out:?}");
        assert_eq!(out, "safe [31mred[0m text\n");

        let out = render("```\n\x1b]52;c;cGF5bG9hZA==\x07\n```\n");
        assert!(!out.contains('\x1b'), "got {out:?}");
        assert!(!out.contains('\x07'), "got {out:?}");
    }

    #[test]
    fn headings_lose_their_hashes() {
        assert_eq!(render("# One\n## Two\n"), "One\nTwo\n");
    }

    #[test]
    fn fence_prints_code_without_markers() {
        let out = render("```rust\nlet x = 1;\n```\nafter\n");
        assert!(out.contains("let x = 1;"));
        assert!(!out.contains("```"));
        assert!(out.ends_with("after\n"), "got {out:?}");
    }

    #[test]
    fn bullets_and_quotes() {
        assert_eq!(render("- item\n> quoted\n"), "- item\n> quoted\n");
    }

    #[test]
    fn single_line_answer_is_flushed() {
        assert_eq!(render("one short sentence"), "one short sentence");
    }

    #[test]
    fn reflow_does_not_drop_characters() {
        let mut out = Vec::new();
        {
            let mut s = Stream::new(&mut out);
            s.feed("Hello wor").unwrap();
            s.feed("Hello").unwrap();
            s.feed("Hello world\n").unwrap();
            s.finish().unwrap();
        }
        assert_eq!(String::from_utf8(out).unwrap(), "Hello world\n");
    }

    #[test]
    fn rewritten_utf8_prefix_does_not_panic() {
        let mut out = Vec::new();
        {
            let mut stream = Stream::new(&mut out);
            stream.feed("abcd\n").unwrap();
            stream.feed("ééé\n").unwrap();
            stream.finish().unwrap();
        }
        assert_eq!(String::from_utf8(out).unwrap(), "abcd\n");
    }

    #[test]
    fn incremental_feeds_match_single_feed() {
        let text = "# Head\n\nSome **bold** text.\n\n```sh\nls -la\n```\n";
        let whole = render(text);
        let mut out = Vec::new();
        {
            let mut s = Stream::new(&mut out);
            for end in [3usize, 10, 17, 30, text.len()] {
                let end = end.min(text.len());
                s.feed(&text[..end]).unwrap();
            }
            s.feed(text).unwrap();
            s.finish().unwrap();
        }
        assert_eq!(String::from_utf8(out).unwrap(), whole);
    }
}
