# deepseek-chat

DeepSeek in your terminal.

![deepseek-chat in a terminal](preview.png)

## What it is

A terminal front end for DeepSeek's web chat. The assistant you already use in
the browser, streamed as text into your shell.

There is no API key, no billing, and no third-party service in the middle. It
opens the real <https://chat.deepseek.com> in a browser you own, keeps its own
login, types your message into the real composer and prints the answer back.
Markdown survives the trip: headings, lists, emphasis and code blocks.

It is built for *talking* to DeepSeek — asking questions, thinking out loud,
working through something — not for wiring a model into an editor, an agent
loop or a script.

## Why this exists

Talking to a model from a terminal usually means one of two things: paying for
API tokens, or opening a browser tab. This is a third option that keeps the
free web experience but moves it to where the work actually happens — the shell.

Plainly, so there is no ambiguity about what this project is for:

- It is for a person chatting. One message, one answer, at a human pace.
- It is **not** a botting tool. Nothing here runs unattended, sends in bulk,
  farms accounts, scrapes conversations, or pools sessions together.
- It does **not** bypass DeepSeek's limits, paywalls or authentication. You
  sign in with your own account, and the free tier you see in the browser is
  the same free tier you get here.
- It **does not** call private endpoints. It drives the visible page, the same
  way your hands would.

The point is convenience, not extraction. Whether it counts as acceptable is
DeepSeek's call, not ours — if you want something unambiguously sanctioned for
programmatic work, use their API at <https://platform.deepseek.com>. It is
free to create a key and it is the right tool for anything automated.

## How it works

1. `--login` opens a normal browser window on a profile of its own. Nothing is
   attached to that window, so DeepSeek treats it like any other browser and
   the sign-in behaves normally, including device checks and CAPTCHAs.
2. The session is made persistent in that profile and verified by loading the
   chat page once. Later runs reuse it, headless, without a window.
3. A message is typed into the real composer and sent with a real keypress,
   with small pauses, rather than fired at the network directly.
4. Thinking mode and web search are switched on through the two chips under
   the composer, and can be changed at any time with `/think` and `/search`.
5. The answer is re-read from the page as it renders, converted back from the
   page's HTML into Markdown, and streamed to stdout complete line by line.
6. Output is plain stdout: styled on a terminal, raw Markdown when piped, and
   never inside an alternate screen. Scrollback, `grep` and redirects work.
7. Conversations are remembered in a small local index so `/sessions` can list
   them and `/sessions N` can reopen one.
8. One conversation at a time. When idle, nothing runs except the browser
   holding your page.

## Usage

### Signing in, once

```bash
deepseek-chat --login
```

Sign in in the window that opens, then close it — or press `enter` in the
terminal if it keeps running in the background. The command prints
`✓ signed in and verified` when the session works. From then on no window is
needed.

### Chatting

```bash
deepseek-chat                            # interactive
deepseek-chat -p "why is the sky blue"   # one answer, then exit
echo "summarise this in 3 bullets" | deepseek-chat
```

| Key | Action |
| --- | --- |
| `enter` | send |
| `\` at the end of a line | continue on the next line |
| `up` / `down` | previous prompts |
| `ctrl+r` | search history |
| `ctrl+c` | quit |

| Command | Action |
| --- | --- |
| `/new` | start a new conversation |
| `/sessions` | list conversations started here |
| `/sessions N` | reopen conversation N |
| `/think [on\|off]` | thinking mode, and showing the reasoning |
| `/search [on\|off]` | web search |
| `/reload` | reload the page |
| `/browser` | raise the browser window |
| `/clear` | clear the screen |
| `/quit` | exit |

| Flag | Action |
| --- | --- |
| `-p`, `--print <TEXT>` | one prompt, print the answer, exit |
| `--login` | sign in with a plain browser window |
| `--headed` | show the browser window |
| `--headless` | no window (default) |
| `--think` | also print the reasoning |
| `--no-color` | plain text, no ANSI |
| `--probe` | print page diagnostics and exit |
| `--say <TEXT>` | with `--probe`, send this and stream the answer |
| `--url <URL>` | drive a different chat page |
| `--chrome <PATH>` | browser binary to use |

### Examples

```bash
deepseek-chat -p "explain the difference between TCP and UDP, briefly"
deepseek-chat -p "rewrite this email to sound less annoyed" < draft.txt

# reasoning, when you want to see how it got there
deepseek-chat --think -p "is 0.1 + 0.2 == 0.3 in floating point?"

# clean Markdown into a file
deepseek-chat -p "write a short guide to brewing pour-over coffee" > coffee.md
```

### When something looks wrong

```bash
deepseek-chat --probe --say "reply with pong"
```

This runs the whole pipeline in a plain terminal and prints what it sees: login
state, the composer, the mode chips, the cookies it can read, the last answer,
and a fingerprint summary. It tells you whether a failure is auth, a selector,
or the network.

## Installation

**Requirements:** Rust 1.88+ and Chromium or Google Chrome on `PATH`.

```bash
cargo install --git https://github.com/batusaribay/deepseek-chat.git --locked
```

The binary is installed as `deepseek-chat`.

## Uninstall

```bash
cargo uninstall deepseek-chat
rm -rf ~/.config/deepseek-chat ~/.local/share/deepseek-chat
```

## Security

- The browser profile is its own, under `~/.local/share/deepseek-chat`. Your
  own browser profile is never opened, read, copied or modified.
- The saved session lives in `session.json` with mode `0600`; the config
  directory is `0700`.
- No telemetry, analytics, or background polling.
- `--probe` prints cookie names and lengths, never their values.

## Files

```text
src/main.rs      CLI, flags, startup
src/cli.rs       the prompt, streaming output, slash commands
src/session.rs   browser control, page reading, mode chips, cookies
src/render.rs    Markdown to terminal, wrapping and stream handling
src/sessions.rs  local index behind /sessions
src/config.rs    paths and settings
preview.png      README preview image
```

## Development

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

## Known Issues

- It is browser automation. DeepSeek can change the page at any time, and a
  CAPTCHA can still decide it does not like the browser.
- Answer extraction depends on the page's HTML. If a release renames the answer
  block, `--probe` shows the candidates and `answer_selector` fixes it without
  a rebuild.
- `/sessions` lists conversations used through this tool, not every
  conversation in your web account.
- Only the parts of the page this tool understands are exposed: no file
  uploads, model picker, or sidebar.
- One conversation at a time.

## Future Ideas

- Import the web account's conversation list into `/sessions`
- File and image attachment support
- Optional JSON output for scripts

## License

This project is licensed under the [MIT License](LICENSE).
