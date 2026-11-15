//! Drives a real, logged-in Chromium over the DevTools protocol.
//!
//! No API key, no cookie copying: the browser itself holds the session, and we
//! only (a) type into the visible composer and (b) read the rendered answer
//! back out of the DOM. Selectors live in config.toml because DeepSeek's
//! frontend class names change from time to time.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
use chromiumoxide::page::Page;
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::config::{Config, profile_dir};

/// Opening a debugging port makes Chromium report `navigator.webdriver === true`,
/// which is what turns DeepSeek's sign-in into an unwinnable CAPTCHA. A real
/// browser reports `false`, so that is what we report.
const STEALTH_JS: &str = r#"
  try {
    Object.defineProperty(Navigator.prototype, 'webdriver', {
      get: () => false,
      configurable: true,
    });
  } catch (e) {}
"#;

pub type Tx = UnboundedSender<Ev>;
pub type CmdTx = UnboundedSender<Cmd>;
pub type Rx = UnboundedReceiver<Ev>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Connecting,
    WaitingLogin,
    Ready,
    Sending,
    Streaming,
}

#[derive(Debug, Clone)]
pub enum Cmd {
    Send(String),
    NewChat,
    Reload,
    Focus,
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum Ev {
    Status(Status),
    Login(bool),
    Answer {
        text: String,
        think: String,
        done: bool,
    },
    Toast(String),
    Fail(String),
    /// The page never became usable; the CLI should stop waiting.
    Fatal(String),
}

#[derive(Debug, Clone, Default, Deserialize)]
struct Snapshot {
    #[serde(default)]
    composer: bool,
    #[serde(rename = "composerText", default)]
    composer_text: String,
    #[serde(default)]
    count: usize,
    #[serde(default)]
    answer: String,
    #[serde(default)]
    think: String,
    #[serde(rename = "loginForm", default)]
    login_form: bool,
    #[serde(default)]
    busy: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    href: String,
}

fn js(cfg: &Config) -> Js {
    Js {
        composer: serde_json::to_string(&cfg.composer_selector).unwrap_or_else(|_| "\"\"".into()),
        answer: serde_json::to_string(&cfg.answer_selector).unwrap_or_else(|_| "\"\"".into()),
        think: serde_json::to_string(&cfg.think_selector).unwrap_or_else(|_| "\"\"".into()),
    }
}

struct Js {
    composer: String,
    answer: String,
    think: String,
}

const VIS: &str = r#"
  const vis = (el) => {
    if (!el) return false;
    const r = el.getBoundingClientRect();
    if (r.width <= 1 || r.height <= 1) return false;
    const s = getComputedStyle(el);
    return s.visibility !== 'hidden' && s.display !== 'none' && s.opacity !== '0';
  };
"#;

impl Js {
    fn snapshot(&self) -> String {
        let body = r#"() => {
__VIS__
  const comps = Array.from(document.querySelectorAll(__COMPOSER__)).filter(vis);
  const el = comps.length ? comps[comps.length - 1] : null;
  let nodes = Array.from(document.querySelectorAll(__ANSWER__)).filter(vis);
  nodes = nodes.filter((n) => !nodes.some((m) => m !== n && m.contains(n)));
  const last = nodes.length ? nodes[nodes.length - 1] : null;
  let thinkText = '';
  if (last) {
    let scope = last;
    for (let i = 0; i < 3 && scope.parentElement; i++) scope = scope.parentElement;
    const ts = Array.from(scope.querySelectorAll(__THINK__)).filter(vis);
    if (ts.length) thinkText = ts[ts.length - 1].innerText || '';
  }
  const btns = Array.from(document.querySelectorAll('button,[role="button"]')).filter(vis);
  const busy = btns.some((b) => {
    const label = ((b.getAttribute('aria-label') || '') + ' ' + (b.innerText || '')).toLowerCase();
    return /stop generating|stop responding|停止生成/.test(label) && !b.disabled;
  });
  return {
    composer: !!el,
    composerText: el ? (el.value !== undefined && el.value !== null ? el.value : (el.innerText || '')) : '',
    count: nodes.length,
    answer: last ? (last.innerText || '') : '',
    think: thinkText,
    loginForm: !!document.querySelector('input[type="password"]'),
    busy: busy,
    title: document.title,
    href: location.href
  };
}"#;
        body.replace("__VIS__", VIS)
            .replace("__COMPOSER__", &self.composer)
            .replace("__ANSWER__", &self.answer)
            .replace("__THINK__", &self.think)
    }

    fn insert(&self, text: &str) -> String {
        let literal = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
        let body = r#"() => {
__VIS__
  const text = __TEXT__;
  const comps = Array.from(document.querySelectorAll(__COMPOSER__)).filter(vis);
  const el = comps.length ? comps[comps.length - 1] : null;
  if (!el) return { ok: false, text: '' };
  el.focus();
  const read = () => (el.value !== undefined && el.value !== null) ? el.value : (el.innerText || '');
  const isField = el.tagName === 'TEXTAREA' || el.tagName === 'INPUT';
  const proto = el.tagName === 'INPUT' ? HTMLInputElement.prototype : HTMLTextAreaElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, 'value');
  if (isField) {
    if (setter && setter.set) { setter.set.call(el, ''); } else { el.value = ''; }
    el.dispatchEvent(new Event('input', { bubbles: true }));
    try { el.setSelectionRange(0, 0); } catch (e) {}
  } else {
    const sel = window.getSelection();
    const range = document.createRange();
    range.selectNodeContents(el);
    sel.removeAllRanges();
    sel.addRange(range);
    try { document.execCommand('delete', false, null); } catch (e) {}
  }
  try { document.execCommand('insertText', false, text); } catch (e) {}
  let now = read();
  const needle = text.slice(0, Math.min(16, text.length));
  if (!now || !now.includes(needle)) {
    if (isField) {
      if (setter && setter.set) { setter.set.call(el, text); } else { el.value = text; }
      el.dispatchEvent(new Event('input', { bubbles: true }));
    } else {
      el.textContent = text;
      el.dispatchEvent(new InputEvent('input', { bubbles: true, data: text, inputType: 'insertText' }));
    }
    now = read();
  }
  return { ok: true, text: now };
}"#;
        body.replace("__VIS__", VIS)
            .replace("__COMPOSER__", &self.composer)
            .replace("__TEXT__", &literal)
    }

    fn click_send(&self) -> String {
        let body = r#"() => {
__VIS__
  const btns = Array.from(document.querySelectorAll('button,[role="button"]')).filter(vis);
  const b = btns.find((x) => !x.disabled && /send|发送|提交/i.test((x.getAttribute('aria-label') || '') + ' ' + (x.innerText || '')));
  if (!b) return false;
  b.click();
  return true;
}"#;
        body.replace("__VIS__", VIS)
    }

    fn new_chat(&self) -> String {
        let body = r#"() => {
__VIS__
  const cands = Array.from(document.querySelectorAll('button,a,[role="button"]')).filter(vis);
  const b = cands.find((x) => !x.disabled && /new chat|new conversation|新建对话|新对话|开启新对话/i.test((x.innerText || '') + ' ' + (x.getAttribute('aria-label') || '')));
  if (b) { b.click(); return true; }
  return false;
}"#;
        body.replace("__VIS__", VIS)
    }
}

/// Prefer the browser the user actually runs, so version and fingerprint match.
fn default_browser() -> Option<String> {
    [
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/usr/bin/google-chrome-stable",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).exists())
    .map(|p| p.to_string())
}

/// Small unpredictable pauses, like a person using a page.
async fn human_pause(min_ms: u64, max_ms: u64) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let span = max_ms.saturating_sub(min_ms).max(1);
    let ms = min_ms + nanos % span;
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

fn desktop_user_agent(product: &str) -> Option<String> {
    let version = product.split('/').nth(1)?;
    Some(format!(
        "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{version} Safari/537.36"
    ))
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SavedCookie {
    name: String,
    value: String,
    domain: String,
    path: String,
    #[serde(default)]
    secure: bool,
    #[serde(default)]
    http_only: bool,
}

pub fn session_file() -> std::path::PathBuf {
    crate::config::data_root().join("session.json")
}

pub struct Session {
    browser: Browser,
    page: Page,
    cfg: Config,
    js: Js,
}

impl Session {
    async fn open(cfg: Config) -> Result<Self> {
        let profile = profile_dir();
        std::fs::create_dir_all(&profile)
            .with_context(|| format!("creating profile dir {}", profile.display()))?;

        // The library's default args include --enable-automation, which sets
        // navigator.webdriver and makes DeepSeek's CAPTCHA loop forever. We
        // build the arg list ourselves instead.
        let mut builder = BrowserConfig::builder()
            .user_data_dir(&profile)
            .window_size(cfg.window_width, cfg.window_height)
            .disable_default_args()
            .arg("disable-background-networking")
            .arg("disable-background-timer-throttling")
            .arg("disable-backgrounding-occluded-windows")
            .arg("disable-renderer-backgrounding")
            .arg("disable-dev-shm-usage")
            .arg("disable-popup-blocking")
            .arg("no-first-run")
            .arg("no-default-browser-check")
            .arg("hide-crash-restore-bubble")
            .arg("lang=en_US");

        builder = if cfg.headless {
            builder.new_headless_mode()
        } else {
            builder.with_head()
        };
        let browser = cfg.chrome.clone().or_else(default_browser);
        if let Some(path) = browser {
            builder = builder.chrome_executable(path);
        }

        let config = builder.build().map_err(|e| anyhow!(e))?;
        let (browser, mut handler) = Browser::launch(config)
            .await
            .context("could not launch Chromium")?;

        tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });

        // Open about:blank first so the stealth patch is installed before the
        // real page ever runs a single line of JavaScript.
        let page = browser
            .new_page("about:blank")
            .await
            .context("could not open a browser tab")?;
        if cfg.stealth {
            page.evaluate_on_new_document(STEALTH_JS).await.ok();
        }

        if cfg.headless {
            // Headless reports "HeadlessChrome" in the UA, which sites treat as
            // a bot. Pretend to be the same version in its normal form.
            if let Ok(version) = browser.version().await
                && let Some(ua) = desktop_user_agent(&version.product)
            {
                page.set_user_agent(ua).await.ok();
            }
        }

        let session = Self {
            browser,
            page,
            js: js(&cfg),
            cfg,
        };
        session.load_cookies().await.ok();

        session
            .page
            .goto(session.cfg.url.clone())
            .await
            .context("could not open the chat page")?;
        session.page.bring_to_front().await.ok();
        Ok(session)
    }

    async fn snapshot(&self) -> Result<Snapshot> {
        let value = self.page.evaluate(self.js.snapshot()).await?;
        value
            .into_value::<Snapshot>()
            .context("unexpected page state")
    }

    /// What a bot detector would see, for `--probe`.
    async fn fingerprint(&self) -> Result<serde_json::Value> {
        let js = r#"() => {
  const webgl = (() => {
    try {
      const c = document.createElement('canvas').getContext('webgl');
      const d = c.getExtension('WEBGL_debug_renderer_info');
      return d ? c.getParameter(d.UNMASKED_RENDERER_WEBGL) : 'n/a';
    } catch (e) { return 'error'; }
  })();
  return {
    webdriver: String(navigator.webdriver),
    hasChromeRuntime: typeof window.chrome === 'object' && !!window.chrome.runtime,
    plugins: navigator.plugins.length,
    languages: (navigator.languages || []).join(','),
    platform: navigator.platform,
    hardwareConcurrency: navigator.hardwareConcurrency,
    userAgent: navigator.userAgent,
    webglRenderer: webgl,
    notifications: Notification.permission
  };
}"#;
        Ok(self.page.evaluate(js).await?.into_value()?)
    }

    /// Cookies the browser can actually decrypt, for `--probe`.
    async fn cookies(&self) -> Result<serde_json::Value> {
        let all = self.page.get_cookies().await?;
        let rows: Vec<serde_json::Value> = all
            .iter()
            .filter(|c| c.domain.contains("deepseek"))
            .map(|c| {
                serde_json::json!({
                    "name": c.name,
                    "domain": c.domain,
                    "httpOnly": c.http_only,
                    "chars": c.value.chars().count(),
                })
            })
            .collect();
        Ok(serde_json::Value::Array(rows))
    }

    /// Elements that look like a composer or an answer, for `--probe`.
    async fn candidates(&self) -> Result<serde_json::Value> {
        let body = r#"() => {
__VIS__
  const sel = 'textarea,[contenteditable="true"],[role="textbox"],[class*="markdown"],[class*="input"],[class*="chat"]';
  const els = Array.from(document.querySelectorAll(sel)).slice(0, 60);
  return els.map((el) => ({
    tag: el.tagName.toLowerCase(),
    id: el.id || '',
    cls: (el.getAttribute('class') || '').slice(0, 140),
    chars: ((el.innerText || el.value || '') + '').length,
    visible: vis(el)
  }));
}"#;
        let js = body.replace("__VIS__", VIS);
        Ok(self.page.evaluate(js).await?.into_value()?)
    }

    async fn wait_ready(&self, tx: &Tx) -> Result<()> {
        let start = Instant::now();
        let mut announced_login = false;
        loop {
            if let Ok(snap) = self.snapshot().await {
                if snap
                    .title
                    .contains("ERROR: The request could not be satisfied")
                {
                    return Err(anyhow!(
                        "DeepSeek's edge blocked the request — run `deepseek-chat --headed` once and retry"
                    ));
                }
                if snap.composer {
                    if announced_login {
                        let _ = tx.send(Ev::Toast("signed in — you're good to go".into()));
                    }
                    let _ = tx.send(Ev::Login(false));
                    return Ok(());
                }
                if snap.login_form {
                    if !announced_login {
                        announced_login = true;
                        let _ = tx.send(Ev::Status(Status::WaitingLogin));
                        let _ = tx.send(Ev::Login(true));
                        let hint = if self.cfg.headless {
                            "not signed in — run `deepseek-chat --headed` once to log in"
                        } else {
                            "log in inside the Chromium window, then this continues automatically"
                        };
                        let _ = tx.send(Ev::Toast(hint.to_string()));
                        self.page.bring_to_front().await.ok();
                    }
                } else if start.elapsed() > Duration::from_secs(90) {
                    return Err(anyhow!(
                        "the chat page never showed a message box (at {})",
                        snap.href
                    ));
                }
            }
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
    }

    /// Save the decrypted session cookies to the project's own file. Chromium batches
    /// cookie writes and we kill it on exit, so relying on its database alone
    /// would occasionally lose a fresh login.
    async fn save_cookies(&self) -> Result<usize> {
        let cookies = self.page.get_cookies().await?;
        let saved: Vec<SavedCookie> = cookies
            .iter()
            .filter(|c| c.domain.contains("deepseek"))
            .map(|c| SavedCookie {
                name: c.name.clone(),
                value: c.value.clone(),
                domain: c.domain.clone(),
                path: c.path.clone(),
                secure: c.secure,
                http_only: c.http_only,
            })
            .collect();
        if saved.is_empty() {
            return Ok(0);
        }

        let path = session_file();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
        }
        std::fs::write(&path, serde_json::to_vec_pretty(&saved)?)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(saved.len())
    }

    /// Put the saved cookies back before the page loads.
    async fn load_cookies(&self) -> Result<usize> {
        use chromiumoxide::cdp::browser_protocol::network::{CookieParam, TimeSinceEpoch};
        let path = session_file();
        let Ok(raw) = std::fs::read(&path) else {
            return Ok(0);
        };
        let saved: Vec<SavedCookie> = serde_json::from_slice(&raw).unwrap_or_default();
        let expires = TimeSinceEpoch::new(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs_f64())
                .unwrap_or(0.0)
                + 180.0 * 86_400.0,
        );

        let mut restored = 0;
        for cookie in saved {
            // Setting by URL works from about:blank, where domain-based
            // Secure cookies are refused.
            let param = CookieParam::builder()
                .name(cookie.name)
                .value(cookie.value)
                .url(self.cfg.url.clone())
                .secure(cookie.secure)
                .http_only(cookie.http_only)
                .expires(expires.clone())
                .build();
            if let Ok(param) = param
                && self.page.set_cookie(param).await.is_ok()
            {
                restored += 1;
            }
        }
        Ok(restored)
    }

    /// Close the browser politely so it writes its cookie database to disk.
    /// Killing it instead loses any cookie it rotated during the session.
    async fn close(mut self) {
        if let Ok(cmd) = self.browser.close().await {
            let _ = cmd;
            self.browser.wait().await.ok();
        }
    }

    async fn press_enter(&self) -> Result<()> {
        let down = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyDown)
            .key("Enter")
            .code("Enter")
            .text("\r")
            .windows_virtual_key_code(13)
            .native_virtual_key_code(13)
            .build()
            .map_err(|e| anyhow!("{e:?}"))?;
        self.page.execute(down).await?;

        let up = DispatchKeyEventParams::builder()
            .r#type(DispatchKeyEventType::KeyUp)
            .key("Enter")
            .code("Enter")
            .windows_virtual_key_code(13)
            .native_virtual_key_code(13)
            .build()
            .map_err(|e| anyhow!("{e:?}"))?;
        self.page.execute(up).await?;
        Ok(())
    }

    async fn send(&self, text: &str, tx: &Tx) -> Result<()> {
        let before = self.snapshot().await?;
        if !before.composer {
            return Err(anyhow!(
                "message box not found — is the page still loading?"
            ));
        }

        human_pause(120, 280).await;

        let inserted: serde_json::Value = self
            .page
            .evaluate(self.js.insert(text))
            .await?
            .into_value()
            .context("composer rejected the text")?;

        let ok = inserted
            .get("ok")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let now = inserted
            .get("text")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !ok {
            return Err(anyhow!("no visible message box to type into"));
        }
        let needle: String = text.chars().take(16).collect();
        if !now.contains(needle.trim()) && !needle.trim().is_empty() {
            return Err(anyhow!(
                "the composer did not accept the text (page may have changed)"
            ));
        }

        human_pause(180, 420).await;
        self.press_enter().await?;

        // Wait for a new answer node to appear. If Enter did nothing, try the
        // send button once.
        let deadline = Instant::now() + Duration::from_secs(25);
        let mut clicked = false;
        loop {
            let snap = self.snapshot().await?;
            if snap.count > before.count {
                break;
            }
            if !clicked && Instant::now().duration_since(deadline) < Duration::from_secs(18) {
                let sent: bool = self
                    .page
                    .evaluate(self.js.click_send())
                    .await?
                    .into_value()
                    .unwrap_or(false);
                if sent {
                    clicked = true;
                }
            }
            if Instant::now() >= deadline {
                let left = snap.composer_text.trim();
                return Err(anyhow!(
                    "no answer appeared (composer still holds {} chars)",
                    left.chars().count()
                ));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        let _ = tx.send(Ev::Status(Status::Streaming));
        let poll = Duration::from_millis(self.cfg.poll_ms.max(40));
        let settle = Duration::from_millis(self.cfg.settle_ms.max(300));
        let hard_stop = Instant::now() + Duration::from_secs(600);

        let mut last_text = String::new();
        let mut last_think = String::new();
        let mut last_change = Instant::now();
        let mut ever_changed = false;

        loop {
            let snap = match self.snapshot().await {
                Ok(s) => s,
                Err(_) => {
                    tokio::time::sleep(poll).await;
                    continue;
                }
            };
            if snap.answer != last_text {
                last_text = snap.answer.clone();
                last_think = snap.think.clone();
                last_change = Instant::now();
                ever_changed = true;
                let _ = tx.send(Ev::Answer {
                    text: last_text.clone(),
                    think: last_think.clone(),
                    done: false,
                });
            } else if snap.think != last_think {
                last_think = snap.think.clone();
                last_change = Instant::now();
            }

            if ever_changed
                && !last_text.trim().is_empty()
                && last_change.elapsed() >= settle
                && !snap.busy
            {
                break;
            }
            if Instant::now() >= hard_stop {
                break;
            }
            if !ever_changed && last_change.elapsed() > Duration::from_secs(90) {
                return Err(anyhow!("the answer never started arriving"));
            }
            tokio::time::sleep(poll).await;
        }

        let _ = tx.send(Ev::Answer {
            text: last_text,
            think: last_think,
            done: true,
        });
        Ok(())
    }

    async fn new_chat(&self) -> Result<()> {
        let clicked: bool = self
            .page
            .evaluate(self.js.new_chat())
            .await?
            .into_value()
            .unwrap_or(false);
        if !clicked {
            self.page.goto(self.cfg.url.clone()).await?;
        }
        tokio::time::sleep(Duration::from_millis(600)).await;
        Ok(())
    }
}

pub fn spawn(cfg: Config) -> (CmdTx, Rx) {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let (ev_tx, ev_rx) = unbounded_channel();
    tokio::spawn(run(cfg, cmd_rx, ev_tx));
    (cmd_tx, ev_rx)
}

async fn run(cfg: Config, mut cmd_rx: UnboundedReceiver<Cmd>, tx: Tx) {
    let _ = tx.send(Ev::Status(Status::Connecting));

    let session = loop {
        match Session::open(cfg.clone()).await {
            Ok(s) => break s,
            Err(e) => {
                let _ = tx.send(Ev::Fail(format!("{e:#}")));
                let _ = tx.send(Ev::Toast("retrying in 3s…".into()));
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
        }
    };

    match session.wait_ready(&tx).await {
        Ok(()) => {
            session.save_cookies().await.ok();
            let _ = tx.send(Ev::Status(Status::Ready));
        }
        Err(e) => {
            let _ = tx.send(Ev::Fatal(format!("{e:#}")));
        }
    }

    while let Some(cmd) = cmd_rx.recv().await {
        match cmd {
            Cmd::Send(text) => {
                let _ = tx.send(Ev::Status(Status::Sending));
                match session.send(&text, &tx).await {
                    Ok(()) => {
                        // Cookies may rotate while chatting; keep our copy fresh.
                        session.save_cookies().await.ok();
                        let _ = tx.send(Ev::Status(Status::Ready));
                    }
                    Err(e) => {
                        let _ = tx.send(Ev::Fail(format!("{e:#}")));
                        let _ = tx.send(Ev::Answer {
                            text: String::new(),
                            think: String::new(),
                            done: true,
                        });
                        let _ = tx.send(Ev::Status(Status::Ready));
                    }
                }
            }
            Cmd::NewChat => {
                if let Err(e) = session.new_chat().await {
                    let _ = tx.send(Ev::Fail(format!("new chat failed: {e:#}")));
                } else {
                    let _ = tx.send(Ev::Toast("new conversation".into()));
                }
            }
            Cmd::Reload => {
                let _ = tx.send(Ev::Status(Status::Connecting));
                let _ = session.page.reload().await;
                match session.wait_ready(&tx).await {
                    Ok(()) => {
                        let _ = tx.send(Ev::Status(Status::Ready));
                    }
                    Err(e) => {
                        let _ = tx.send(Ev::Fail(format!("{e:#}")));
                    }
                }
            }
            Cmd::Focus => {
                session.page.bring_to_front().await.ok();
                let _ = tx.send(Ev::Toast("browser window raised".into()));
            }
            Cmd::Shutdown => {
                session.save_cookies().await.ok();
                session.close().await;
                break;
            }
        }
    }
}

/// `deepseek-chat --probe`: report what the page looks like, then exit. Handy after a
/// DeepSeek frontend update, when a selector needs adjusting.
/// Sign in with a browser that has *nothing* automated attached to it.
///
/// DeepSeek treats a browser driven over the DevTools protocol as a new device
/// and answers the sign-in with a CAPTCHA tied to emailed codes, which is
/// unwinnable from an automation window. So for login we spawn a plain
/// Chromium on dsx's own profile, let the user sign in as they would anywhere,
/// and when the window closes we make the session cookie persistent.
pub async fn login(cfg: Config) -> Result<()> {
    use std::io::IsTerminal;

    let profile = profile_dir();
    std::fs::create_dir_all(&profile).with_context(|| format!("creating {}", profile.display()))?;

    if let Some(pid) = live_profile_pid(&profile) {
        bail!("the browser is already open with this profile (pid {pid}) — close it first");
    }

    let browser = cfg
        .chrome
        .clone()
        .or_else(default_browser)
        .unwrap_or_else(|| "chromium".to_string());

    println!("Opening a normal {browser} window — no debugging port, no automation.");
    println!("Sign in the way you usually do, then close the window.");
    println!("If the window stays open in the background, press enter here instead.\n");

    let mut child = tokio::process::Command::new(&browser)
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg(&cfg.url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("could not start {browser}"))?;

    let db = profile.join("Default").join("Cookies");

    let exited = if std::io::stdin().is_terminal() {
        let enter = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
        });
        tokio::select! {
            _ = child.wait() => true,
            _ = enter => false,
        }
    } else {
        loop {
            if child.try_wait().ok().flatten().is_some() {
                break true;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };

    if !exited {
        shutdown(&mut child).await;
    }

    if !db.exists() {
        bail!("no browser profile was written — did the browser start?");
    }

    // Mark the session cookies persistent so the next start keeps them.
    {
        let conn =
            rusqlite::Connection::open(&db).with_context(|| format!("opening {}", db.display()))?;
        persist_deepseek_cookies(&conn)?;
    }

    // The profile is authoritative now; an older copy would only interfere.
    let _ = std::fs::remove_file(session_file());

    println!("checking the session…");
    let mut verify = cfg.clone();
    verify.headless = true;
    let session = Session::open(verify).await?;

    let mut signed_in = false;
    for _ in 0..30 {
        if let Ok(snap) = session.snapshot().await
            && snap.composer
        {
            signed_in = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    if signed_in {
        session.save_cookies().await.ok();
        println!("\n✓ signed in and verified. Now just run `deepseek-chat`.");
        Ok(())
    } else {
        bail!(
            "that did not produce a signed-in session.\n\
             Run `deepseek-chat --login` again and sign in until the chat page \n\
             itself is open, then close the window."
        )
    }
}

/// Ask the browser to exit so it flushes its cookie database, then insist.
async fn shutdown(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status();
    }
    if tokio::time::timeout(Duration::from_secs(8), child.wait())
        .await
        .is_err()
    {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

/// A session cookie is dropped by the next browser start. Turn the ones we
/// found into persistent cookies, in place: the encrypted values are left
/// exactly as they are, only the expiry metadata changes.
fn persist_deepseek_cookies(conn: &rusqlite::Connection) -> Result<usize> {
    let future = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + 116_444_736_000
        + 180 * 86_400;

    Ok(conn.execute(
        "update cookies set expires_utc = ?1, has_expires = 1, is_persistent = 1
         where host_key like '%deepseek%' and (expires_utc = 0 or has_expires = 0)",
        [future * 1_000_000],
    )?)
}

fn live_profile_pid(profile: &std::path::Path) -> Option<u32> {
    let target = std::fs::read_link(profile.join("SingletonLock")).ok()?;
    let pid: u32 = target.to_string_lossy().rsplit('-').next()?.parse().ok()?;

    // A leftover lock from a process that already exited (or is a zombie) is
    // not a running browser.
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let state = stat.rsplit(')').next()?.split_whitespace().next()?;
    (state != "Z").then_some(pid)
}

pub async fn probe(cfg: Config, say: Option<String>) -> Result<()> {
    println!("launching Chromium…");
    let session = Session::open(cfg.clone()).await?;
    println!("waiting for the chat page…");

    let start = Instant::now();
    loop {
        if let Ok(snap) = session.snapshot().await
            && (snap.composer || start.elapsed() > Duration::from_secs(60))
        {
            let mut report = snapshot_json(&snap);
            report["elements"] = session
                .candidates()
                .await
                .unwrap_or(serde_json::Value::Null);
            report["fingerprint"] = session
                .fingerprint()
                .await
                .unwrap_or(serde_json::Value::Null);
            report["cookies"] = session.cookies().await.unwrap_or(serde_json::Value::Null);
            println!("{}", serde_json::to_string_pretty(&report)?);

            if let Some(prompt) = say {
                println!("\nsending: {prompt}");
                let (tx, mut rx) = unbounded_channel();
                let printer = tokio::spawn(async move {
                    let mut shown = 0usize;
                    while let Some(ev) = rx.recv().await {
                        if let Ev::Answer { text, done, .. } = ev {
                            if text.len() > shown || done {
                                shown = text.len();
                                println!("[{} chars] {}", text.chars().count(), tail(&text, 300));
                            }
                            if done {
                                break;
                            }
                        }
                    }
                });
                let result = session.send(&prompt, &tx).await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                drop(tx);
                let _ = printer.await;
                result?;
                println!("\npipeline OK");
            }
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(70) {
            return Err(anyhow!("timed out waiting for the page"));
        }
        tokio::time::sleep(Duration::from_millis(700)).await;
    }
}

fn tail(text: &str, n: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = chars.len().saturating_sub(n);
    let mut out: String = chars[start..].iter().collect();
    out = out.replace('\n', " \\n ");
    if start > 0 {
        out.insert(0, '…');
    }
    out
}

fn snapshot_json(s: &Snapshot) -> serde_json::Value {
    serde_json::json!({
        "url": s.href,
        "title": s.title,
        "loggedIn": s.composer && !s.login_form,
        "composerFound": s.composer,
        "loginFormFound": s.login_form,
        "answerNodes": s.count,
        "busy": s.busy,
        "composerText": s.composer_text,
        "answerPreview": s.answer.chars().take(400).collect::<String>(),
        "thinkPreview": s.think.chars().take(400).collect::<String>(),
    })
}

#[cfg(test)]
mod tests {
    use super::persist_deepseek_cookies;

    #[test]
    fn session_cookies_become_persistent() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "create table cookies (
                host_key text, name text, expires_utc integer,
                has_expires integer, is_persistent integer
            );
            insert into cookies values ('chat.deepseek.com', 'ds_session_id', 0, 0, 0);
            insert into cookies values ('example.com', 'keep', 0, 0, 0);",
        )
        .unwrap();

        assert_eq!(persist_deepseek_cookies(&conn).unwrap(), 1);

        let (expires, has, persistent): (i64, i64, i64) = conn
            .query_row(
                "select expires_utc, has_expires, is_persistent from cookies
                 where name = 'ds_session_id'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(expires > 0);
        assert_eq!((has, persistent), (1, 1));

        let untouched: i64 = conn
            .query_row(
                "select expires_utc from cookies where name = 'keep'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(untouched, 0);
    }
}
