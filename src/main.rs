mod cli;
mod config;
mod render;
mod session;

use std::io::{self, IsTerminal, Read};
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

use config::Config;

/// Chat with DeepSeek in your terminal.
///
/// Drives chat.deepseek.com in a real browser that keeps its own login, so
/// there is no API key and no third-party server in the middle. Answers stream
/// as text and pipe like any other command.
#[derive(Parser, Debug)]
#[command(name = "deepseek-chat", version, about, long_about = None)]
struct Args {
    /// Send one prompt, print the answer, then exit
    #[arg(short = 'p', long = "print", value_name = "TEXT")]
    print: Option<String>,

    /// The question to ask (same as --print)
    #[arg(value_name = "QUESTION")]
    question: Option<String>,

    /// Sign in with a plain browser window (needed once)
    #[arg(long)]
    login: bool,

    /// Run the browser with no window (default)
    #[arg(long, conflicts_with = "headed")]
    headless: bool,

    /// Show the browser window
    #[arg(long)]
    headed: bool,

    /// Also print the model's reasoning when it thinks
    #[arg(long)]
    think: bool,

    /// Plain text output, without ANSI styling
    #[arg(long = "no-color")]
    no_color: bool,

    /// Print what the page looks like, then exit
    #[arg(long)]
    probe: bool,

    /// With --probe: also send this text and stream the answer
    #[arg(long, value_name = "TEXT", requires = "probe")]
    say: Option<String>,

    /// Render a Markdown file the way answers are rendered
    #[arg(long, value_name = "FILE", hide = true)]
    render: Option<PathBuf>,

    /// Drive a different chat page
    #[arg(long, value_name = "URL")]
    url: Option<String>,

    /// Browser binary to use
    #[arg(long, value_name = "PATH")]
    chrome: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut cfg = Config::load().unwrap_or_default();

    if args.headless {
        cfg.headless = true;
    }
    if args.headed || args.login {
        cfg.headless = false;
    }
    if let Some(url) = &args.url {
        cfg.url = url.clone();
    }
    if let Some(chrome) = &args.chrome {
        cfg.chrome = Some(chrome.clone());
    }

    let color = cli::use_color(args.no_color);

    if let Some(path) = args.render {
        let text = std::fs::read_to_string(&path)?;
        let mut stream = render::Stream::new(io::stdout(), color);
        stream.feed(&text)?;
        stream.finish()?;
        println!();
        return Ok(());
    }

    let mut prompt = args.print.or(args.question);

    // Piped stdin means one shot: `echo "why is the sky blue" | deepseek-chat`
    if prompt.is_none() && !io::stdin().is_terminal() {
        let mut buffer = String::new();
        io::stdin().read_to_string(&mut buffer)?;
        if !buffer.trim().is_empty() {
            prompt = Some(buffer.trim().to_string());
        }
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    if args.login {
        let result = runtime.block_on(session::login(cfg));
        runtime.shutdown_timeout(std::time::Duration::from_millis(500));
        return result;
    }

    if args.probe {
        let result = runtime.block_on(session::probe(cfg, args.say));
        runtime.shutdown_timeout(std::time::Duration::from_millis(500));
        return result;
    }

    let result = runtime.block_on(async move {
        let app = cli::Cli::start(cfg, color, args.think);
        match prompt {
            Some(prompt) => app.once(prompt).await,
            None => app.run().await,
        }
    });

    // Shut the worker down first so the browser process is reaped with us.
    runtime.shutdown_timeout(std::time::Duration::from_millis(500));

    match result {
        Ok(()) => Ok(()),
        Err(e) if e.to_string() == "interrupted" => std::process::exit(130),
        Err(e) => {
            eprintln!("deepseek-chat: {e:#}");
            std::process::exit(1);
        }
    }
}
