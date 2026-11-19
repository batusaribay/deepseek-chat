//! A small local index of conversations, so `/sessions` works without
//! scraping the website's sidebar.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::data_root;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub updated: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub entries: Vec<Entry>,
}

fn path() -> PathBuf {
    data_root().join("sessions.json")
}

pub fn load() -> Index {
    let Ok(raw) = std::fs::read(path()) else {
        return Index::default();
    };
    serde_json::from_slice(&raw).unwrap_or_default()
}

pub fn list() -> Vec<Entry> {
    load().entries
}

/// Conversation titles are shown in the terminal. Strip control characters so
/// a title that came from piped input cannot inject escape sequences.
pub fn safe_title(title: &str) -> String {
    title
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(80)
        .collect()
}

/// Remember a conversation. Newest first, capped so the file stays small.
pub fn record(id: &str, title: &str) {
    if id.is_empty() {
        return;
    }
    let mut index = load();
    index.entries.retain(|entry| entry.id != id);
    index.entries.insert(
        0,
        Entry {
            id: id.to_string(),
            title: safe_title(title),
            updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        },
    );
    index.entries.truncate(200);

    if let Ok(raw) = serde_json::to_vec_pretty(&index) {
        let _ = write_private(&path(), &raw);
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }

    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sessions.json".to_string());
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));

    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> std::io::Result<()> {
        let mut file = options.open(&tmp)?;
        file.write_all(bytes)?;
        file.flush()
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    #[cfg(windows)]
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path)
}

/// Pull the conversation id out of a chat URL.
pub fn id_from_url(url: &str) -> Option<&str> {
    let rest = url.split("/chat/s/").nth(1)?;
    let id = rest.split(['?', '#', '/']).next().unwrap_or("");
    (!id.is_empty()).then_some(id)
}
