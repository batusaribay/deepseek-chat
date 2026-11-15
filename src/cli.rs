//! Terminal front end: a prompt, streamed answers, and nothing else.
//!
//! No alternate screen, no full-screen app. Output goes to stdout, so it pipes,
//! redirects, and scrolls back like any other command.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::config::{Config, data_root};
use crate::render::Stream;
use crate::session::{self, Cmd, CmdTx, Ev, Status};

const PROMPT: &str = "› ";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";
const RED: &str = "\x1b[31m";

pub struct Cli {
    cfg: Config,
    tx: CmdTx,
    ev: session::Rx,
    color: bool,
    think: bool,
    signed_in: bool,
}

fn tty_write(msg: &str) {
    if io::stderr().is_terminal() {
        let mut err = io::stderr();
        let _ = write!(err, "\r\x1b[K{msg}");
        let _ = err.flush();
    }
}

fn tty_clear() {
    if io::stderr().is_terminal() {
        let mut err = io::stderr();
        let _ = write!(err, "\r\x1b[K");
        let _ = err.flush();
    }
}

impl Cli {
    pub fn start(cfg: Config, color: bool, think: bool) -> Self {
        let (tx, ev) = session::spawn(cfg.clone());
        Self {
            cfg,
            tx,
            ev,
            color,
            think,
            signed_in: false,
        }
    }

    /// Block until the browser is on the page and the composer exists.
    async fn wait_ready(&mut self) -> Result<()> {
        let mut hinted = false;
        let mut login_since: Option<std::time::Instant> = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        let ctrl = tokio::signal::ctrl_c();
        tokio::pin!(ctrl);
        loop {
            if std::time::Instant::now() > deadline {
                tty_clear();
                bail!("timed out starting the browser");
            }
            let ev = tokio::select! {
                _ = &mut ctrl => bail!("interrupted"),
                r = tokio::time::timeout(Duration::from_secs(30), self.ev.recv()) => r,
            };
            match ev {
                Err(_) => continue,
                Ok(None) => bail!("the browser session ended"),
                Ok(Some(ev)) => match ev {
                    Ev::Status(Status::Ready) => {
                        tty_clear();
                        self.signed_in = true;
                        if hinted {
                            eprintln!("{GREEN}✓ signed in{RESET} {DIM}(session saved){RESET}");
                        }
                        return Ok(());
                    }
                    Ev::Status(Status::Connecting) => tty_write("starting browser…"),
                    Ev::Status(Status::WaitingLogin) => {
                        tty_clear();
                        login_since.get_or_insert_with(std::time::Instant::now);
                        if self.cfg.headless
                            && login_since.is_some_and(|at| at.elapsed() > Duration::from_secs(5))
                        {
                            bail!(
                                "not signed in — run `deepseek-chat --login` once to sign in\n\
                                 (the browser needs a window for that; after that headless works)"
                            );
                        }
                        if !hinted {
                            hinted = true;
                            eprintln!(
                                "{YELLOW}Sign in in the browser window.{RESET} \
                                 {DIM}This continues on its own once the chat box appears.{RESET}\n\
                                 {DIM}If a CAPTCHA keeps failing, use your password instead of the emailed code.{RESET}"
                            );
                        }
                    }
                    Ev::Login(true) if !hinted => {
                        hinted = true;
                        tty_clear();
                        eprintln!(
                            "{YELLOW}Sign in in the browser window.{RESET} \
                             {DIM}This continues on its own once the chat box appears.{RESET}\n\
                             {DIM}If a CAPTCHA keeps failing, use your password instead of the emailed code.{RESET}"
                        );
                    }
                    Ev::Fatal(msg) => {
                        tty_clear();
                        self.shutdown().await;
                        bail!("{msg}");
                    }
                    Ev::Fail(msg) => {
                        tty_clear();
                        eprintln!("{RED}! {msg}{RESET}");
                    }
                    _ => {}
                },
            }
        }
    }

    /// Read a full message, honouring trailing backslashes for continuation.
    fn read_message(rl: &mut DefaultEditor) -> Result<Option<String>> {
        let mut buffer = String::new();
        loop {
            let prompt = if buffer.is_empty() { PROMPT } else { "  " };
            match rl.readline(prompt) {
                Ok(line) => {
                    if let Some(stripped) = line.strip_suffix('\\') {
                        buffer.push_str(stripped);
                        buffer.push('\n');
                        continue;
                    }
                    buffer.push_str(&line);
                    if buffer.trim().is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(buffer));
                }
                Err(ReadlineError::Interrupted) => {
                    println!();
                    return Ok(Some("/quit".into()));
                }
                Err(ReadlineError::Eof) => return Ok(Some("/quit".into())),
                Err(e) => return Err(e.into()),
            }
        }
    }

    async fn send(&mut self, text: &str) -> Result<bool> {
        if self.tx.send(Cmd::Send(text.to_string())).is_err() {
            bail!("the browser session ended");
        }

        let mut stream = Stream::new(io::stdout(), self.color);
        let mut think = String::new();
        let mut got_answer = false;
        tty_write("thinking…");

        let ctrl = tokio::signal::ctrl_c();
        tokio::pin!(ctrl);

        loop {
            let next = tokio::select! {
                _ = &mut ctrl => {
                    tty_clear();
                    return Ok(false);
                }
                ev = self.ev.recv() => ev,
            };
            match next {
                None => {
                    tty_clear();
                    bail!("the browser session ended");
                }
                Some(Ev::Answer {
                    text,
                    think: t,
                    done,
                }) => {
                    if !t.is_empty() {
                        think = t;
                    }
                    if text.len() > stream.consumed() {
                        tty_clear();
                    }
                    stream.feed(&text)?;
                    if !text.trim().is_empty() {
                        got_answer = true;
                    }
                    if done {
                        break;
                    }
                }
                Some(Ev::Status(Status::Streaming)) => tty_write("answering…"),
                Some(Ev::Status(Status::Sending)) => tty_write("sending…"),
                Some(Ev::Status(Status::Ready)) => {}
                Some(Ev::Status(Status::Connecting)) => tty_write("reconnecting…"),
                Some(Ev::Status(Status::WaitingLogin)) => tty_clear(),
                Some(Ev::Fail(msg)) | Some(Ev::Fatal(msg)) => {
                    tty_clear();
                    eprintln!("{RED}! {msg}{RESET}");
                }
                Some(Ev::Toast(msg)) => {
                    tty_clear();
                    eprintln!("{DIM}{msg}{RESET}");
                }
                Some(Ev::Login(_)) => {}
            }
        }

        tty_clear();
        stream.finish()?;
        if !got_answer {
            println!("{RED}(no answer captured — try `deepseek-chat --probe`){RESET}");
        }
        if self.think && !think.trim().is_empty() {
            println!();
            println!("{DIM}[reasoning]{RESET}");
            println!("{DIM}{}{RESET}", think.trim());
        }
        println!();
        Ok(true)
    }

    async fn command(&mut self, line: &str) -> Result<bool> {
        let (name, arg) = match line.trim_start_matches('/').split_once(' ') {
            Some((n, a)) => (n.trim(), a.trim()),
            None => (line.trim_start_matches('/').trim(), ""),
        };
        match name {
            "quit" | "q" | "exit" => return Ok(true),
            "new" => {
                let _ = self.tx.send(Cmd::NewChat);
                println!("{DIM}new conversation{RESET}\n");
            }
            "reload" => {
                self.tx.send(Cmd::Reload).ok();
                println!("{DIM}reloading the page{RESET}\n");
            }
            "browser" | "focus" => {
                self.tx.send(Cmd::Focus).ok();
                println!("{DIM}browser raised{RESET}\n");
            }
            "think" => {
                self.think = match arg {
                    "on" => true,
                    "off" => false,
                    _ => !self.think,
                };
                println!(
                    "{DIM}reasoning {}{RESET}\n",
                    if self.think { "on" } else { "off" }
                );
            }
            "clear" => {
                print!("\x1b[2J\x1b[H");
                let _ = io::stdout().flush();
            }
            "help" | "" => {
                println!(
                    "{DIM}/new      start a new conversation
/reload   reload the page
/browser  raise the browser window
/think    toggle showing reasoning
/clear    clear the screen
/quit     exit

End a line with \\ to continue on the next one.{RESET}
"
                );
            }
            other => println!("{RED}unknown command /{other}{RESET}\n"),
        }
        Ok(false)
    }

    pub async fn run(mut self) -> Result<()> {
        self.wait_ready().await?;

        let history = history_path();
        let mut rl = DefaultEditor::new()?;
        if let Some(path) = history.as_ref() {
            let _ = rl.load_history(path);
        }

        loop {
            let line = match Self::read_message(&mut rl) {
                Ok(Some(line)) => line,
                Ok(None) => continue,
                Err(e) => {
                    eprintln!("{RED}{e}{RESET}");
                    continue;
                }
            };

            if line.trim_start().starts_with('/') {
                if self.command(&line).await? {
                    break;
                }
                continue;
            }

            let _ = rl.add_history_entry(line.trim_end());
            match self.send(line.trim_end()).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => eprintln!("{RED}! {e:#}{RESET}"),
            }
        }

        if let Some(path) = history {
            let _ = rl.save_history(&path);
        }
        self.shutdown().await;
        if self.color {
            println!("{DIM}bye{RESET}");
        }
        Ok(())
    }

    /// Give the worker a moment to close the browser so cookies are flushed.
    async fn shutdown(&mut self) {
        if self.tx.send(Cmd::Shutdown).is_err() {
            return;
        }
        let _ = tokio::time::timeout(Duration::from_secs(8), async {
            while self.ev.recv().await.is_some() {}
        })
        .await;
    }

    /// One prompt, one answer, exit. Used by `-p` and by piped stdin.
    pub async fn once(mut self, prompt: String) -> Result<()> {
        self.wait_ready().await?;
        let sent = self.send(&prompt).await?;
        self.shutdown().await;
        if sent {
            Ok(())
        } else {
            std::process::exit(130);
        }
    }
}

fn history_path() -> Option<PathBuf> {
    let dir = data_root();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("history.txt"))
}

/// Should output be styled?
pub fn use_color(no_color_flag: bool) -> bool {
    if no_color_flag || std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    io::stdout().is_terminal()
}
