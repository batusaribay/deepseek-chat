# deepseek-chat

A Rust CLI front end for DeepSeek's web chat

![deepseek-chat in a terminal](preview.png)

> Read this first. This automates a website that does not permit
> automation. DeepSeek's terms forbid it, and accounts can be banned — mine
> was suspended for three days while building this. If losing the account
> matters to you, use the official API instead.

## How it works

1. `--login` opens a normal browser window on a profile of its own. Nothing is
   attached to that window, so DeepSeek treats it like any other browser and
   the sign-in behaves normally, including device checks and CAPTCHAs.
2. The session is made persistent in that profile and verified by loading the
   chat page once. Later runs reuse it, headless, without a window. If the
   session expires, the next run opens a window and asks you to sign in.
3. A message is typed into the real composer and sent with a real keypress,
   with small pauses, rather than fired at the network directly.
4. Thinking mode and web search are switched on through the two chips under
   the composer, and can be changed at any time with `/think` and `/search`.
5. The answer is re-read from the page as it renders, converted back from the
   page's HTML into Markdown, and streamed to stdout complete line by line.
6. Output is plain text with no colour or cursor tricks: styled Markdown
   markers are stripped, code is printed as itself, and nothing is drawn inside
   an alternate screen. Scrollback, `grep` and redirects work.
7. Conversations are remembered in a small local index so `/conversations` can
   list them and `/conversations N` can reopen one.
8. One conversation at a time. When idle, nothing runs except the browser
   holding your page.

## Usage

### Signing in, once

```bash
deepseek-chat --login
```

Sign in in the window that opens, then close it — or press `enter` in the
terminal if it keeps running in the background. The command prints
`✓ signed in and verified` when the session works.

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
| `/conversations` | list conversations started here |
| `/conversations N` | reopen conversation N |
| `/new` | start a new conversation |
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
deepseek-chat --probe
```

This prints what the page looks like: login state, the composer, the mode
chips, the cookies it can read, and a fingerprint summary. It tells you whether
a failure is auth, a selector, or the network.

`--probe` is also the polite way to find out whether you are still signed in
without sending a message.

## Install

**Requirements:** Rust 1.88+ and Chromium or Google Chrome on `PATH`.

```bash
cargo install --git https://github.com/batusaribay/deepseek-chat.git --locked
```

## Uninstall

```bash
cargo uninstall deepseek-chat
rm -rf ~/.config/deepseek-chat ~/.local/share/deepseek-chat
```

## Security

- The browser profile is its own, under `~/.local/share/deepseek-chat`. Your
  own browser profile is never opened, read, copied or modified by this
  version.
- No credentials are stored by this tool. The login lives in the browser
  profile Chromium maintains, and the config directory is `0700`.
- No telemetry, analytics, or background polling.
- `--probe` prints cookie names and lengths, never their values.

## License

This project is licensed under the [MIT License](LICENSE).
