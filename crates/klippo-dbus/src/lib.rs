//! klippo-dbus: shared D-Bus interface definitions.
//!
//! Declares the two interfaces exposed by the daemon on `org.klippo.Daemon`
//! at `/org/klippo/Daemon`:
//!
//! * `org.klippo.Daemon1` — control + query surface used by the CLI and UI.
//! * `org.klippo.Capture1` — capture push surface called by the GNOME extension.
//!
//! Only the **client** (proxy) side lives here; the daemon implements the
//! server side against its own state. Sharing the proxies keeps the daemon, the
//! `klippo` CLI, and the GNOME bridge from drifting out of sync.

use serde::{Deserialize, Serialize};
use zvariant::Type;

/// Well-known bus name the daemon owns.
pub const BUS_NAME: &str = "org.klippo.Daemon";

/// Object path hosting both interfaces.
pub const OBJECT_PATH: &str = "/org/klippo/Daemon";

/// Wire form of a history entry returned by `ListEntries` (D-Bus `(sssxb)`).
///
/// Carries just enough to render a row; full content (and image bytes) is
/// fetched on demand via `GetEntryContent`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct DbusEntry {
    /// Content-hash id.
    pub id: String,
    /// `"text"` or `"image"`.
    pub kind: String,
    /// One-line display preview.
    pub preview: String,
    /// Thumbnail path for image entries, or `""` when absent.
    pub thumb_path: String,
    /// Last-used time (unix ms) for MRU ordering.
    pub timestamp_ms: i64,
    pub pinned: bool,
}

/// Control + query interface (CLI `klippo toggle`, and the UI).
#[zbus::proxy(
    interface = "org.klippo.Daemon1",
    default_service = "org.klippo.Daemon",
    default_path = "/org/klippo/Daemon"
)]
pub trait Daemon1 {
    /// Show the popup if hidden, hide it if shown.
    fn toggle(&self) -> zbus::Result<()>;
    fn show(&self) -> zbus::Result<()>;
    fn hide(&self) -> zbus::Result<()>;
    /// Clear the entire history.
    fn clear(&self) -> zbus::Result<()>;
    /// Promote an entry to the top and copy it to the system clipboard
    /// (does **not** paste — matching Klipper).
    fn select(&self, id: &str) -> zbus::Result<()>;
    /// Delete a single entry from history.
    fn remove_entry(&self, id: &str) -> zbus::Result<()>;
    /// Run a named action against an entry's content.
    fn run_action(&self, id: &str, action_name: &str) -> zbus::Result<()>;
    /// Pin or unpin an entry (pinned entries stay above the MRU order).
    fn set_pinned(&self, id: &str, pinned: bool) -> zbus::Result<()>;
    /// The newest `limit` entries, newest first.
    fn list_entries(&self, limit: u32) -> zbus::Result<Vec<DbusEntry>>;
    /// Full content of an entry as `(mime_type, bytes)`.
    fn get_entry_content(&self, id: &str) -> zbus::Result<(String, Vec<u8>)>;
    /// Read a config value (stringified).
    fn get_config(&self, key: &str) -> zbus::Result<String>;
    /// Write a config value (parsed from the string) and persist it.
    fn set_config(&self, key: &str, value: &str) -> zbus::Result<()>;
    /// Liveness/version probe.
    fn ping(&self) -> zbus::Result<String>;

    /// Emitted whenever the history changes (the UI refreshes on this).
    #[zbus(signal)]
    fn history_changed(&self) -> zbus::Result<()>;
    /// Emitted when a config key changes.
    #[zbus(signal)]
    fn config_changed(&self, key: String) -> zbus::Result<()>;
    /// Emitted when matched content should offer an action menu. `actions` is a
    /// list of `(action_name, command_label)`.
    #[zbus(signal)]
    fn action_popup_requested(
        &self,
        id: String,
        actions: Vec<(String, String)>,
    ) -> zbus::Result<()>;
}

/// Capture push interface — called by the GNOME Shell extension (and any other
/// out-of-process capturer). `source` is `"clipboard"` or `"primary"`.
#[zbus::proxy(
    interface = "org.klippo.Capture1",
    default_service = "org.klippo.Daemon",
    default_path = "/org/klippo/Daemon"
)]
pub trait Capture1 {
    fn add_text(&self, text: &str, source: &str) -> zbus::Result<()>;
    fn add_image(&self, mime: &str, bytes: &[u8], source: &str) -> zbus::Result<()>;
    fn clipboard_cleared(&self, source: &str) -> zbus::Result<()>;
    /// Periodic liveness ping so the daemon knows the capturer is alive.
    fn heartbeat(&self) -> zbus::Result<()>;
}
