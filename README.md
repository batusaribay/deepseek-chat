# deepseek-chat

A Rust CLI to chat with DeepSeek

![deepseek-chat in a terminal](preview.png)

## Why this exists

DeepSeek has no keyless terminal client. This one drives the real web chat in a
browser profile of its own, so there is no API key and no third-party server in
the middle. The cost is that it automates a site that does not permit
automation: DeepSeek's terms forbid it, and accounts can be suspended. Use the
official API if losing the account matters.

## How it works

`--login` opens a normal browser window on a private profile. Nothing is
attached to that window, so sign-in, device checks, and CAPTCHAs behave as they
do in any browser. The session is persisted in that profile and verified by
loading the chat page once. Later runs reuse it headless, and an expired session
opens a window and asks for a sign-in.

A message is typed into the real composer and sent with a real keypress, with
short pauses, instead of calling the network directly. Thinking mode and web
search are switched through the two chips under the composer and can be changed
with `/think` and `/search`. The answer is re-read from the page as it renders,
converted from HTML back to plain text, and streamed complete line by line.

Output has no colour or cursor tricks. Markdown markers are stripped, code
prints as itself, and nothing is drawn in an alternate screen. Conversation
titles are kept in a small local index so they can be listed and reopened.

## Usage

```text
deepseek-chat [OPTIONS] [QUESTION]

Commands:
  /conversations      List conversations started here
  /conversations N    Reopen conversation N
  /new                Start a new conversation
  /think [on|off]     Thinking mode and reasoning output
  /search [on|off]    Web search
  /reload             Reload the page
  /browser            Raise the browser window
  /clear              Clear the screen
  /quit               Exit

Options:
  -p, --print <TEXT>   Send one prompt, print the answer, and exit
      --chat           Start the interactive chat
      --json           Print one prompt's answer as JSON
      --login          Sign in with a plain browser window
      --headless       Run the browser with no window (default)
      --headed         Show the browser window
      --think          Also print the model's reasoning
      --probe          Print page diagnostics and exit
      --say <TEXT>     With --probe: send this and stream the answer
      --url <URL>      Drive a different chat page
      --chrome <PATH>  Browser binary to use
      --color <WHEN>   Color output: auto, always, or never [default: auto]
  -h, --help           Print help
  -V, --version        Print version
```

```bash
deepseek-chat --chat
deepseek-chat -p "why is the sky blue"
echo "summarise this in 3 bullets" | deepseek-chat
deepseek-chat --think -p "is 0.1 + 0.2 == 0.3 in floating point?"
deepseek-chat --json -p "explain TCP and UDP" | jq -r .answer
```

## Notes

Sign in once with `deepseek-chat --login`. Piped stdin also works without a
terminal. In the chat, `enter` sends, a trailing `\` continues the line, and
`ctrl+c` quits.

## Config

Reads `~/.config/deepseek-chat/config.toml` and creates it on first use. Flags
override the file; defaults apply otherwise.

## Install

**Requirements:** Rust 1.89+, `git`, and Chromium or Google Chrome on `PATH`.

```bash
cargo install --git https://github.com/batusaribay/deepseek-chat.git --locked
```

## License

This project is licensed under the [MIT License](LICENSE).
