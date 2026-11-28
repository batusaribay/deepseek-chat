use std::env;
use std::io::IsTerminal;
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    enabled: bool,
}

#[derive(Clone, Copy)]
pub enum Role {
    Muted,
    Error,
}

static THEME: OnceLock<Theme> = OnceLock::new();

pub fn init() {
    let _ = THEME.set(Theme::from_env());
}

pub fn current() -> Theme {
    *THEME.get().unwrap_or(&Theme::plain())
}

impl Theme {
    fn from_env() -> Self {
        let clicolor = env::var("CLICOLOR").ok();
        let force = env::var("CLICOLOR_FORCE").ok();
        let term = env::var("TERM").ok();
        Self {
            enabled: color_enabled(
                env::var_os("NO_COLOR").is_some(),
                clicolor.as_deref(),
                force.as_deref(),
                term.as_deref(),
                std::io::stdout().is_terminal(),
            ),
        }
    }

    const fn plain() -> Self {
        Self { enabled: false }
    }

    pub fn paint(self, role: Role, text: &str) -> String {
        if !self.enabled {
            return text.to_string();
        }
        let code = match role {
            Role::Muted => "2",
            Role::Error => "31",
        };
        format!("\x1b[{code}m{text}\x1b[0m")
    }
}

fn color_enabled(
    no_color: bool,
    clicolor: Option<&str>,
    force: Option<&str>,
    term: Option<&str>,
    tty: bool,
) -> bool {
    if no_color {
        return false;
    }
    if force.is_some_and(|value| !value.is_empty() && value != "0") {
        return true;
    }
    if clicolor == Some("0") || term == Some("dumb") {
        return false;
    }
    tty
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_respects_the_environment() {
        assert!(color_enabled(false, None, None, Some("xterm"), true));
        assert!(!color_enabled(false, None, None, Some("xterm"), false));
        assert!(!color_enabled(true, None, None, Some("xterm"), true));
        assert!(!color_enabled(false, Some("0"), None, Some("xterm"), true));
        assert!(!color_enabled(false, None, None, Some("dumb"), true));
        assert!(color_enabled(
            false,
            Some("0"),
            Some("1"),
            Some("dumb"),
            false
        ));
    }
}
