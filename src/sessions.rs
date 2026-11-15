//! A small local index of conversations, so `/sessions` works without
//! scraping the website's sidebar.

use std::path::PathBuf;

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
            title: title.trim().chars().take(80).collect(),
            updated: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        },
    );
    index.entries.truncate(200);

    if let Some(dir) = path().parent() {
        std::fs::create_dir_all(dir).ok();
    }
    if let Ok(raw) = serde_json::to_vec_pretty(&index) {
        std::fs::write(path(), raw).ok();
    }
}

/// Pull the conversation id out of a chat URL.
pub fn id_from_url(url: &str) -> Option<&str> {
    let rest = url.split("/chat/s/").nth(1)?;
    let id = rest.split(['?', '#', '/']).next().unwrap_or("");
    (!id.is_empty()).then_some(id)
}
