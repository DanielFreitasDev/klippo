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

/// Clipboard features available for a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackendCapabilities {
    pub text: bool,
    pub image: bool,
    pub primary: bool,
    pub sync_clipboards: bool,
}

/// Capabilities expected from a backend in normal operation.
pub fn backend_capabilities(kind: BackendKind) -> BackendCapabilities {
    match kind {
        BackendKind::GnomeBridge => BackendCapabilities {
            text: true,
            image: true,
            primary: false,
            sync_clipboards: false,
        },
        BackendKind::WaylandDataControl | BackendKind::X11 => BackendCapabilities {
            text: true,
            image: true,
            primary: true,
            sync_clipboards: true,
        },
        BackendKind::None => BackendCapabilities {
            text: false,
            image: false,
            primary: false,
            sync_clipboards: false,
        },
    }
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

fn linux_selection(source: Source) -> arboard::LinuxClipboardKind {
    match source {
        Source::Primary => arboard::LinuxClipboardKind::Primary,
        Source::Clipboard => arboard::LinuxClipboardKind::Clipboard,
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
            let mut clipboard_state = SelectionState::default();
            let mut primary_state = SelectionState::default();
            loop {
                if !poll_x11_selection(&mut clipboard, &tx, Source::Clipboard, &mut clipboard_state)
                {
                    break;
                }
                if !poll_x11_selection(&mut clipboard, &tx, Source::Primary, &mut primary_state) {
                    break;
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

#[derive(Default)]
struct SelectionState {
    last_text: String,
    last_image: String,
    was_empty: bool,
}

fn poll_x11_selection(
    clipboard: &mut arboard::Clipboard,
    tx: &mpsc::Sender<ClipboardEvent>,
    source: Source,
    state: &mut SelectionState,
) -> bool {
    use arboard::GetExtLinux;

    let selection = linux_selection(source);
    if let Ok(text) = clipboard.get().clipboard(selection).text() {
        if !text.is_empty() && text != state.last_text {
            state.last_text = text.clone();
            state.last_image.clear();
            state.was_empty = false;
            return tx
                .blocking_send(ClipboardEvent::Text { text, source })
                .is_ok();
        }
        return true;
    }

    if let Ok(image) = clipboard.get().clipboard(selection).image() {
        let bytes = match image_data_to_png(image) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(error = %e, "X11: could not encode clipboard image");
                return true;
            }
        };
        let hash = klippo_core::model::hash_bytes(&bytes);
        if hash != state.last_image {
            state.last_text.clear();
            state.last_image = hash;
            state.was_empty = false;
            return tx
                .blocking_send(ClipboardEvent::Image {
                    mime: "image/png".to_string(),
                    bytes,
                    source,
                })
                .is_ok();
        }
        return true;
    }

    if !state.was_empty {
        state.last_text.clear();
        state.last_image.clear();
        state.was_empty = true;
        return tx.blocking_send(ClipboardEvent::Cleared { source }).is_ok();
    }
    true
}

fn image_data_to_png(image: arboard::ImageData<'static>) -> anyhow::Result<Vec<u8>> {
    let rgba = image::RgbaImage::from_raw(
        image.width as u32,
        image.height as u32,
        image.bytes.into_owned(),
    )
    .context("invalid RGBA clipboard image")?;
    let mut bytes = Vec::new();
    image::DynamicImage::ImageRgba8(rgba).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    )?;
    Ok(bytes)
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
        for (mime, subcommand) in [("text/plain", "__feed"), ("image/png", "__feed-image")] {
            std::process::Command::new("wl-paste")
                .arg("--watch")
                .arg("--type")
                .arg(mime)
                .arg(&exe)
                .arg(subcommand)
                .spawn()
                .with_context(|| {
                    "failed to start `wl-paste --watch` (is wl-clipboard installed?)"
                })?;
        }
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

    #[test]
    fn capabilities_match_backend_scope() {
        assert!(backend_capabilities(BackendKind::X11).primary);
        assert!(backend_capabilities(BackendKind::WaylandDataControl).sync_clipboards);
        assert!(!backend_capabilities(BackendKind::GnomeBridge).primary);
        assert!(!backend_capabilities(BackendKind::None).text);
    }
}
