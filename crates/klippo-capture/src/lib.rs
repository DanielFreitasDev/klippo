//! klippo-capture: platform clipboard backends behind a common trait.
//!
//! A [`ClipboardSource`] monitors clipboard changes and forwards
//! [`ClipboardEvent`]s; a [`ClipboardWriter`] sets the system clipboard.
//! [`detect_backend`] chooses the right backend for the current session.
//!
//! On GNOME Wayland the source is a no-op ([`NullSource`]) because Mutter
//! exposes no data-control protocol — events instead arrive over D-Bus from the
//! GNOME Shell extension. Writing the clipboard there is done by the focused
//! GTK popup, so the writer is also a no-op on that path.

use anyhow::Context;
use async_trait::async_trait;
use tokio::sync::mpsc;

pub use klippo_core::Source;

/// A clipboard change observed by a [`ClipboardSource`].
#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    Text {
        text: String,
        source: Source,
    },
    Image {
        mime: String,
        bytes: Vec<u8>,
        source: Source,
    },
    Cleared {
        source: Source,
    },
}

/// Monitors a clipboard and forwards changes until cancelled (by aborting the
/// task running it).
#[async_trait]
pub trait ClipboardSource: Send {
    /// Run the monitor loop, sending events on `tx`. Returns when the source
    /// finishes or errors; long-running sources only return on cancellation.
    async fn run(self: Box<Self>, tx: mpsc::Sender<ClipboardEvent>) -> anyhow::Result<()>;

    /// Human-readable backend name for logging.
    fn name(&self) -> &'static str;
}

/// Sets the system clipboard (used by `Select` on backends where the daemon
/// owns the selection directly: X11, KDE/wlroots Wayland).
pub trait ClipboardWriter: Send + Sync {
    fn set_text(&self, text: &str, source: Source) -> anyhow::Result<()>;
    fn set_image(&self, mime: &str, bytes: &[u8], source: Source) -> anyhow::Result<()>;
}

/// Which capture backend to use for the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// GNOME Wayland: events arrive via D-Bus from the Shell extension.
    GnomeBridge,
    /// KDE 6.4+/wlroots Wayland: ext-data-control / wlr-data-control.
    WaylandDataControl,
    /// X11 (or Xwayland): XFixes selection notifications.
    X11,
    /// No usable backend detected.
    None,
}

/// Decide the backend from the environment.
///
/// Overridable with `KLIPPO_BACKEND=x11|wayland-dc|gnome|none` for testing.
pub fn detect_backend() -> BackendKind {
    if let Ok(forced) = std::env::var("KLIPPO_BACKEND") {
        return match forced.trim() {
            "x11" => BackendKind::X11,
            "wayland-dc" => BackendKind::WaylandDataControl,
            "gnome" => BackendKind::GnomeBridge,
            _ => BackendKind::None,
        };
    }

    let session = std::env::var("XDG_SESSION_TYPE").unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .to_lowercase();

    if session == "wayland" {
        if desktop.contains("gnome") {
            BackendKind::GnomeBridge
        } else {
            BackendKind::WaylandDataControl
        }
    } else if session == "x11" || std::env::var("DISPLAY").is_ok() {
        BackendKind::X11
    } else {
        BackendKind::None
    }
}

/// Parse the D-Bus `source` string used on the wire.
pub fn parse_source(s: &str) -> Source {
    match s {
        "primary" => Source::Primary,
        _ => Source::Clipboard,
    }
}

/// The wire string for a [`Source`].
pub fn source_str(source: Source) -> &'static str {
    match source {
        Source::Primary => "primary",
        Source::Clipboard => "clipboard",
    }
}

/// A source that never emits (GNOME bridge / no-backend placeholder).
pub struct NullSource;

#[async_trait]
impl ClipboardSource for NullSource {
    async fn run(self: Box<Self>, _tx: mpsc::Sender<ClipboardEvent>) -> anyhow::Result<()> {
        std::future::pending::<()>().await;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "null"
    }
}

/// A writer that does nothing (GNOME path writes via the focused GTK popup).
pub struct NullWriter;

impl ClipboardWriter for NullWriter {
    fn set_text(&self, _text: &str, _source: Source) -> anyhow::Result<()> {
        Ok(())
    }
    fn set_image(&self, _mime: &str, _bytes: &[u8], _source: Source) -> anyhow::Result<()> {
        Ok(())
    }
}

/// X11 clipboard monitor. Polls the CLIPBOARD selection via `arboard` on a
/// dedicated thread (real X11 sessions; under Xwayland `arboard` may pick the
/// Wayland backend, which GNOME doesn't allow — there the GNOME bridge is used).
#[derive(Default)]
pub struct X11Source;

impl X11Source {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ClipboardSource for X11Source {
    async fn run(self: Box<Self>, tx: mpsc::Sender<ClipboardEvent>) -> anyhow::Result<()> {
        // arboard is blocking; poll on a dedicated OS thread and forward changes.
        std::thread::spawn(move || {
            let mut clipboard = match arboard::Clipboard::new() {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "X11: could not open the clipboard");
                    return;
                }
            };
            let mut last = String::new();
            loop {
                if let Ok(text) = clipboard.get_text() {
                    if !text.is_empty() && text != last {
                        last = text.clone();
                        let event = ClipboardEvent::Text {
                            text,
                            source: Source::Clipboard,
                        };
                        if tx.blocking_send(event).is_err() {
                            break; // consumer dropped → daemon shutting down
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
        Ok(())
    }

    fn name(&self) -> &'static str {
        "x11-poll"
    }
}

/// Wayland data-control monitor for KDE 6.4+/wlroots/Sway. Delegates to
/// `wl-paste --watch`, which runs `klippo __feed` on each change (that pushes to
/// the daemon over D-Bus). Requires `wl-clipboard` to be installed.
#[derive(Default)]
pub struct WaylandDataControlSource;

impl WaylandDataControlSource {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ClipboardSource for WaylandDataControlSource {
    async fn run(self: Box<Self>, _tx: mpsc::Sender<ClipboardEvent>) -> anyhow::Result<()> {
        let exe = std::env::current_exe().context("locating the klippo executable")?;
        std::process::Command::new("wl-paste")
            .arg("--watch")
            .arg(&exe)
            .arg("__feed")
            .spawn()
            .context("failed to start `wl-paste --watch` (is wl-clipboard installed?)")?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "wayland-data-control (wl-paste)"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_roundtrip() {
        assert_eq!(parse_source(source_str(Source::Primary)), Source::Primary);
        assert_eq!(
            parse_source(source_str(Source::Clipboard)),
            Source::Clipboard
        );
        assert_eq!(parse_source("anything-else"), Source::Clipboard);
    }
}
