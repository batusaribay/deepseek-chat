use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Everything tunable lives in ~/.config/deepseek-chat/config.toml.
///
/// The selectors are here on purpose: when DeepSeek ships a new frontend, this
/// file is the only thing that needs editing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The chat page to drive. Keep the trailing slash.
    pub url: String,
    /// Show a browser window. Signing in needs one; afterwards it can stay off.
    pub headless: bool,
    /// Hide the artefacts of remote debugging (fixes most stubborn CAPTCHAs).
    pub stealth: bool,
    /// Explicit browser binary. `None` uses the one you run day to day.
    pub chrome: Option<String>,
    pub window_width: u32,
    pub window_height: u32,
    /// How often the answer is re-read from the page, in milliseconds.
    pub poll_ms: u64,
    /// Text must be unchanged this long before an answer counts as finished.
    pub settle_ms: u64,
    /// Where the message goes.
    pub composer_selector: String,
    /// What counts as a rendered assistant answer.
    pub answer_selector: String,
    /// The model's reasoning block, when thinking mode is on.
    pub think_selector: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: "https://chat.deepseek.com/".into(),
            headless: true,
            stealth: true,
            chrome: None,
            window_width: 960,
            window_height: 900,
            poll_ms: 120,
            settle_ms: 1200,
            composer_selector: "textarea, [contenteditable=\"true\"], [role=\"textbox\"]".into(),
            answer_selector: ".ds-markdown, [class*=\"ds-markdown\"], [class*=\"markdown\"]".into(),
            think_selector: "[class*=\"think\"], [class*=\"reason\"]".into(),
        }
    }
}

const HEADER: &str = "\
# deepseek-chat configuration
#
# Nothing here needs an API key. These settings control the browser session
# and, if DeepSeek redesigns its site, which elements are used.

";

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            let cfg = Config::default();
            cfg.save().ok();
            return Ok(cfg);
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&raw).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).ok();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
            }
        }
        std::fs::write(&path, format!("{HEADER}{}", toml::to_string_pretty(self)?))
            .with_context(|| format!("writing {}", path.display()))
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("deepseek-chat")
        .join("config.toml")
}

pub fn data_root() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("deepseek-chat")
}

/// A dedicated browser profile, so the login survives restarts and your own
/// browser profile is never read, written or locked.
pub fn profile_dir() -> PathBuf {
    data_root().join("chromium-profile")
}
