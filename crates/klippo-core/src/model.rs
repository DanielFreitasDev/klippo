//! The clipboard history entry model.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Which selection a clipboard event came from.
///
/// On X11 these are two independent buffers (PRIMARY = mouse selection,
/// CLIPBOARD = explicit Ctrl+C). Klipper can track either or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Clipboard,
    Primary,
}

/// Whether an entry holds text or an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Text,
    Image,
}

/// Stable identifier for a history entry: the hex BLAKE3 hash of its content.
///
/// Using the content hash as the primary key makes deduplication automatic —
/// copying identical content twice maps to the same id.
pub type EntryId = String;

/// Maximum number of characters kept in a row preview.
const PREVIEW_MAX_CHARS: usize = 200;

/// A single clipboard history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// Content hash (hex), also the primary key.
    pub id: EntryId,
    pub kind: EntryKind,
    /// Present for [`EntryKind::Text`].
    pub text: Option<String>,
    /// Path to the stored PNG for [`EntryKind::Image`].
    pub image_path: Option<PathBuf>,
    /// Path to the stored thumbnail PNG for [`EntryKind::Image`].
    pub thumb_path: Option<PathBuf>,
    /// One-line display text (text flattened/truncated, or "Imagem WxH").
    pub preview: String,
    /// Last-used time in unix milliseconds; drives MRU ordering.
    pub timestamp_ms: i64,
    /// Reserved for a future pin feature (kept above the MRU order when true).
    pub pinned: bool,
}

impl Entry {
    /// Build a text entry, computing its id and preview.
    pub fn new_text(text: impl Into<String>, timestamp_ms: i64) -> Entry {
        let text = text.into();
        Entry {
            id: hash_bytes(text.as_bytes()),
            kind: EntryKind::Text,
            preview: make_preview(&text),
            text: Some(text),
            image_path: None,
            thumb_path: None,
            timestamp_ms,
            pinned: false,
        }
    }

    /// Build an image entry from an already-computed content hash, on-disk paths
    /// and pixel dimensions.
    pub fn new_image(
        content_hash: String,
        image_path: PathBuf,
        thumb_path: PathBuf,
        width: u32,
        height: u32,
        timestamp_ms: i64,
    ) -> Entry {
        Entry {
            id: content_hash,
            kind: EntryKind::Image,
            text: None,
            image_path: Some(image_path),
            thumb_path: Some(thumb_path),
            preview: format!("Imagem {width}×{height}"),
            timestamp_ms,
            pinned: false,
        }
    }
}

/// Hex BLAKE3 hash of arbitrary bytes.
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Current time in unix milliseconds.
pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Flatten whitespace/newlines to single spaces and truncate for a row preview.
pub fn make_preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out: String = flat.chars().take(PREVIEW_MAX_CHARS).collect();
    if flat.chars().count() > PREVIEW_MAX_CHARS {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_text_has_identical_id() {
        assert_eq!(Entry::new_text("hi", 1).id, Entry::new_text("hi", 9).id);
        assert_ne!(Entry::new_text("hi", 1).id, Entry::new_text("ho", 1).id);
    }

    #[test]
    fn preview_flattens_and_truncates() {
        assert_eq!(make_preview("  a\n\tb   c "), "a b c");
        let long = "x".repeat(500);
        let p = make_preview(&long);
        assert!(p.ends_with('…'));
        assert_eq!(p.chars().count(), PREVIEW_MAX_CHARS + 1);
    }
}
