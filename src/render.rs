//! Streams an answer into a normal terminal: no alternate screen, no cursor
//! games, no flicker. Text is styled as it arrives and the terminal's own
//! wrapping does the layout, so output stays selectable and pipeable.
//!
//! When colour is off (not a TTY, or `--no-color`), the raw Markdown is passed
//! through untouched so `deepseek-chat -p ... > notes.md` gives clean Markdown.

use std::io::{self, Write};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const BOLD_OFF: &str = "\x1b[22m";
const ITALIC: &str = "\x1b[3m";
const ITALIC_OFF: &str = "\x1b[23m";
const CODE_FG: &str = "\x1b[38;5;108m";
const CODE_BG: &str = "\x1b[48;5;236m";
const HEAD_FG: &str = "\x1b[38;5;75m";
const DIM: &str = "\x1b[2m";
const DIM_OFF: &str = "\x1b[22m";

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
    color: bool,
    consumed: usize,
    in_fence: bool,
    line_start: bool,
    head: String,
    bold: bool,
    italic: bool,
    heading: bool,
    code: bool,
    /// Collecting the info string of an opening fence.
    opening: Option<String>,
    /// Swallow the rest of a line we already rendered.
    skip_line: bool,
    stars: usize,
    /// Text received but not yet a complete line. Holding it back keeps a
    /// reflowed partial line from being printed half-formed.
    pending: String,
}

impl<W: Write> Stream<W> {
    pub fn new(out: W, color: bool) -> Self {
        Self {
            out,
            color,
            consumed: 0,
            in_fence: false,
            line_start: true,
            head: String::new(),
            bold: false,
            italic: false,
            heading: false,
            code: false,
            opening: None,
            skip_line: false,
            stars: 0,
            pending: String::new(),
        }
    }

    /// How much of the answer has already been written.
    pub fn consumed(&self) -> usize {
        self.consumed
    }

    /// Feed the whole answer so far; only complete lines are written, so a
    /// line that is still being rendered upstream cannot come out garbled.
    pub fn feed(&mut self, full: &str) -> io::Result<()> {
        if full.len() <= self.consumed {
            return Ok(());
        }
        let delta = &full[self.consumed..];
        self.consumed = full.len();
        self.pending.push_str(delta);

        let Some(end) = self.pending.rfind('\n') else {
            return Ok(());
        };
        let ready: String = self.pending.drain(..=end).collect();
        self.emit(&ready)
    }

    fn emit(&mut self, text: &str) -> io::Result<()> {
        if !self.color {
            self.out.write_all(text.as_bytes())?;
            return self.out.flush();
        }
        let chars: Vec<char> = text.chars().collect();
        for ch in chars {
            self.put(ch)?;
        }
        self.out.flush()
    }

    /// Flush anything held back and leave the terminal in a clean state.
    pub fn finish(&mut self) -> io::Result<()> {
        if !self.head.is_empty() {
            let head = std::mem::take(&mut self.head);
            match classify(&head) {
                Some(Block::Fence) => {}
                Some(Block::Heading) => write!(
                    self.out,
                    "{BOLD}{HEAD_FG}{}",
                    head.trim_start_matches('#').trim_start()
                )?,
                _ => self.out.write_all(head.as_bytes())?,
            }
        }
        if self.color {
            self.out.write_all(RESET.as_bytes())?;
        }
        self.out.flush()
    }

    fn put(&mut self, ch: char) -> io::Result<()> {
        if let Some(lang) = self.opening.as_mut() {
            if ch == '\n' {
                let lang = std::mem::take(&mut self.opening).unwrap_or_default();
                let lang = lang.trim();
                if lang.is_empty() {
                    writeln!(self.out, "{DIM}╭─{DIM_OFF}{RESET}")?;
                } else {
                    writeln!(self.out, "{DIM}╭─ {lang}{DIM_OFF}{RESET}")?;
                }
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
                    writeln!(self.out, "{RESET}{DIM}╰─{DIM_OFF}{RESET}")?;
                    self.skip_line = true;
                    return Ok(());
                }
                let head = std::mem::take(&mut self.head);
                write!(self.out, "{CODE_BG}{CODE_FG}{head}")?;
                self.line_start = false;
                return Ok(());
            }
            if ch == '\n' {
                writeln!(self.out, "{RESET}")?;
                self.line_start = true;
                return Ok(());
            }
            return write!(self.out, "{CODE_BG}{CODE_FG}{ch}");
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
                self.heading = true;
                write!(
                    self.out,
                    "{BOLD}{HEAD_FG}{}",
                    head.trim_start_matches('#').trim_start()
                )
            }
            Block::Bullet => write!(self.out, "{DIM}• {DIM_OFF}"),
            Block::Quote => write!(self.out, "{DIM}│ {DIM_OFF}"),
            Block::Rule => {
                writeln!(self.out, "{DIM}────────{RESET}")?;
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
        if ch == '\n' {
            if self.heading {
                self.out.write_all(RESET.as_bytes())?;
                self.heading = false;
            }
            if self.code {
                self.out.write_all(RESET.as_bytes())?;
                self.code = false;
            }
            if self.bold {
                self.out.write_all(BOLD_OFF.as_bytes())?;
                self.bold = false;
            }
            if self.italic {
                self.out.write_all(ITALIC_OFF.as_bytes())?;
                self.italic = false;
            }
            self.stars = 0;
            self.out.write_all(b"\n")?;
            self.line_start = true;
            return Ok(());
        }

        match ch {
            '*' => {
                self.stars += 1;
                return Ok(());
            }
            '`' => {
                self.stars = 0;
                self.code = !self.code;
                let seq = if self.code { CODE_FG } else { RESET };
                self.out.write_all(seq.as_bytes())?;
                return Ok(());
            }
            _ => {}
        }

        if self.stars > 0 {
            let n = std::mem::take(&mut self.stars);
            if n >= 2 {
                self.bold = !self.bold;
                let seq = if self.bold { BOLD } else { BOLD_OFF };
                self.out.write_all(seq.as_bytes())?;
            }
            if n % 2 == 1 {
                self.italic = !self.italic;
                let seq = if self.italic { ITALIC } else { ITALIC_OFF };
                self.out.write_all(seq.as_bytes())?;
            }
        }

        let mut buf = [0u8; 4];
        self.out.write_all(ch.encode_utf8(&mut buf).as_bytes())
    }
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

    fn render(text: &str, color: bool) -> String {
        let mut out = Vec::new();
        {
            let mut s = Stream::new(&mut out, color);
            s.feed(text).unwrap();
            s.finish().unwrap();
        }
        String::from_utf8(out).unwrap()
    }

    fn plain(text: &str) -> String {
        let out = render(text, true);
        // Strip ANSI for assertions.
        let mut clean = String::new();
        let mut chars = out.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                clean.push(c);
            }
        }
        clean
    }

    #[test]
    fn classify_heading_variants() {
        assert_eq!(classify("# "), Some(Block::Heading));
        assert_eq!(classify("## "), Some(Block::Heading));
        assert_eq!(classify("### "), Some(Block::Heading));
        assert_eq!(classify("## L"), Some(Block::Heading));
        assert_eq!(classify("#x"), Some(Block::Plain));
    }

    #[test]
    fn classify_blocks() {
        assert_eq!(classify("```"), Some(Block::Fence));
        assert_eq!(classify("```rust"), Some(Block::Fence));
        assert_eq!(classify("- "), Some(Block::Bullet));
        assert_eq!(classify("> "), Some(Block::Quote));
        assert_eq!(classify("---"), Some(Block::Rule));
    }

    #[test]
    fn headings_lose_their_hashes() {
        let out = plain("# One\n## Two\n");
        assert_eq!(out, "One\nTwo\n");
    }

    #[test]
    fn fence_swallows_its_markers() {
        let out = plain("```rust\nlet x = 1;\n```\nafter\n");
        assert!(out.contains("let x = 1;"));
        assert!(!out.contains("```"));
        assert!(out.ends_with("after\n"), "got {out:?}");
    }

    #[test]
    fn bullets_and_quotes() {
        let out = plain("- item\n> quoted\n");
        assert_eq!(out, "• item\n│ quoted\n");
    }

    #[test]
    fn raw_mode_is_untouched() {
        let text = "# h\n**b** `c`\n```\nx\n```\n";
        assert_eq!(render(text, false), text);
    }

    #[test]
    fn bold_and_code_emit_styles() {
        let out = render("**bold** and `code`\n", true);
        assert!(out.contains(BOLD));
        assert!(out.contains(CODE_FG));
    }

    #[test]
    fn incremental_feeds_match_single_feed() {
        let text = "# Head\n\nSome **bold** text.\n\n```sh\nls -la\n```\n";
        let whole = render(text, true);
        let mut out = Vec::new();
        {
            let mut s = Stream::new(&mut out, true);
            for end in [3usize, 10, 17, 30, text.len()] {
                let end = end.min(text.len());
                s.feed(&text[..end]).unwrap();
            }
            s.feed(text).unwrap();
            s.finish().unwrap();
        }
        let split = String::from_utf8(out).unwrap();
        assert_eq!(split, whole);
    }
}
