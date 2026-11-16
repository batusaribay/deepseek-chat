//! Terminal front end: a prompt, streamed answers, and nothing else.
//!
//! No alternate screen, no colours, no full-screen app. Output goes to stdout,
//! so it pipes, redirects and scrolls back like any other command.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Result, bail};
use rustyline::DefaultEditor;
use rustyline::error::ReadlineError;

use crate::config::{Config, data_root};
use crate::render::Stream;
use crate::session::{self, Cmd, CmdTx, Ev, Status};
use crate::sessions;

const PROMPT: &str = "› ";

pub struct Cli {
    cfg: Config,
    tx: CmdTx,
    ev: session::Rx,
    think: bool,
    thinking: bool,
    search: bool,
}

fn tty_write(msg: &str) {
    if io::stderr().is_terminal() {
        let mut err = io::stderr();
        let _ = write!(err, "\r{msg}\x1b[K");
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
    pub fn start(cfg: Config, think: bool) -> Self {
        let thinking = cfg.thinking;
        let search = cfg.search;
        let (tx, ev) = session::spawn(cfg.clone());
        Self {
            cfg,
            tx,
            ev,
            think,
            thinking,
            search,
        }
    }

    /// Block until the browser is on the page and the composer exists.
    async fn wait_ready(&mut self) -> Result<()> {
        let mut login_since: Option<std::time::Instant> = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(240);
        let ctrl = tokio::signal::ctrl_c();
        tokio::pin!(ctrl);

        loop {
            if std::time::Instant::now() > deadline {
                tty_clear();
                bail!("timed out starting the browser");
            }

            let event = tokio::select! {
                _ = &mut ctrl => {
                    self.shutdown().await;
                    return Err(anyhow::anyhow!("interrupted"));
                }
                r = tokio::time::timeout(Duration::from_secs(30), self.ev.recv()) => r,
            };

            match event {
                Err(_) => continue,
                Ok(None) => bail!("the browser session ended"),
                Ok(Some(ev)) => match ev {
                    Ev::Status(Status::Ready) => {
                        tty_clear();
                        return Ok(());
                    }
                    Ev::Status(Status::Connecting) => tty_write("starting browser…"),
                    Ev::Status(Status::WaitingLogin) => {
                        tty_clear();
                        if self.cfg.headless {
                            self.shutdown().await;
                            bail!("login required");
                        }
                        login_since.get_or_insert_with(std::time::Instant::now);
                        if login_since.is_some_and(|at| at.elapsed() > Duration::from_secs(300)) {
                            bail!("still not signed in");
                        }
                        if login_since.is_some_and(|at| at.elapsed() < Duration::from_millis(300)) {
                            println!(
                                "Sign in in the browser window, then this continues on its own."
                            );
                        }
                    }
                    Ev::Fatal(msg) => {
                        tty_clear();
                        self.shutdown().await;
                        bail!("{msg}");
                    }
                    Ev::Fail(msg) => {
                        tty_clear();
                        eprintln!("error: {msg}");
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
                Err(ReadlineError::Eof) => println!(),
                Err(e) => return Err(e.into()),
            }
            return Ok(Some("/quit".into()));
        }
    }

    async fn send(&mut self, text: &str) -> Result<bool> {
        if self.tx.send(Cmd::Send(text.to_string())).is_err() {
            bail!("the browser session ended");
        }

        let mut stream = Stream::new(io::stdout());
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
                Some(Ev::Status(_)) => {}
                Some(Ev::Fail(msg)) | Some(Ev::Fatal(msg)) => {
                    tty_clear();
                    eprintln!("error: {msg}");
                }
                Some(Ev::Toast(msg)) => {
                    tty_clear();
                    eprintln!("{msg}");
                }
            }
        }

        tty_clear();
        stream.finish()?;
        if !got_answer {
            println!("(no answer captured — try `deepseek-chat --probe`)");
        }
        if self.think && !think.trim().is_empty() {
            println!();
            println!("[reasoning]");
            println!("{}", think.trim());
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
                self.thinking = self.cfg.thinking;
                self.search = self.cfg.search;
                println!("new conversation");
            }
            "reload" => {
                self.tx.send(Cmd::Reload).ok();
                println!("reloading the page");
            }
            "browser" | "focus" => {
                self.tx.send(Cmd::Focus).ok();
                println!("browser raised");
            }
            "think" | "thinking" => {
                self.thinking = flag(arg, self.thinking);
                self.think = self.thinking;
                self.apply_modes();
                println!("thinking {}", on_off(self.thinking));
            }
            "search" | "web" => {
                self.search = flag(arg, self.search);
                self.apply_modes();
                println!("web search {}", on_off(self.search));
            }
            "conversations" | "chats" | "sessions" => {
                if arg.is_empty() {
                    self.list_conversations();
                } else if let Ok(n) = arg.parse::<usize>() {
                    let entries = sessions::list();
                    match entries.get(n.wrapping_sub(1)) {
                        Some(entry) => {
                            let _ = self.tx.send(Cmd::Open(entry.id.clone()));
                            println!("opening {}", entry.title);
                        }
                        None => println!("no conversation {n}"),
                    }
                } else {
                    println!("usage: /conversations [number]");
                }
            }
            "clear" => {
                print!("\x1b[2J\x1b[H");
                let _ = io::stdout().flush();
            }
            "help" | "" => {
                println!(
                    "/conversations        list conversations started here
/conversations N      reopen conversation N
/new                  start a new conversation
/think [on|off]       thinking mode
/search [on|off]      web search
/reload               reload the page
/browser              raise the browser window
/clear                clear the screen
/quit                 exit

End a line with \\ to continue on the next one."
                );
            }
            other => println!("unknown command /{other}"),
        }
        Ok(false)
    }

    fn apply_modes(&self) {
        let _ = self.tx.send(Cmd::SetMode {
            thinking: self.thinking,
            search: self.search,
        });
    }

    fn list_conversations(&self) {
        let entries = sessions::list();
        if entries.is_empty() {
            println!("no conversations yet");
            return;
        }
        for (i, entry) in entries.iter().take(20).enumerate() {
            println!("{:3}  {}", i + 1, entry.title);
        }
        println!("\n/conversations N to reopen one");
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
                    eprintln!("{e}");
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
                Err(e) => {
                    if e.to_string() == "login required" {
                        self.shutdown().await;
                        return Err(e);
                    }
                    eprintln!("! {e:#}");
                }
            }
        }

        if let Some(path) = history {
            let _ = rl.save_history(&path);
        }
        self.shutdown().await;
        Ok(())
    }

    /// One prompt, one answer, exit.
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
}

fn history_path() -> Option<PathBuf> {
    let dir = data_root();
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir.join("history.txt"))
}

fn flag(arg: &str, current: bool) -> bool {
    match arg {
        "on" | "1" | "yes" | "true" => true,
        "off" | "0" | "no" | "false" => false,
        _ => !current,
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
