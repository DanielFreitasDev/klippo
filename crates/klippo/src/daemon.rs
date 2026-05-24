//! The Klippo daemon: shared state, the two D-Bus interfaces, and process wiring.
//!
//! Threading model: GTK owns the **main thread** (see [`crate::ui`]); a
//! background multi-threaded Tokio runtime hosts the zbus service and (later)
//! the capture backends. The two communicate through:
//!
//! * an `async-channel` of [`UiEvent`]s (daemon → UI), awaited on the GTK main
//!   loop, and
//! * direct, synchronous [`AppState`] store calls (UI → state), since SQLite
//!   operations are fast and `AppState` is `Send + Sync`.
//!
//! `AppState` is the single owner of the history store and config.

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::Instant;

use tracing::{error, info, warn};
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

use klippo_capture::{detect_backend, parse_source, BackendKind};
use klippo_core::config::{ColorScheme, Config};
use klippo_core::model::{now_ms, Entry, EntryKind};
use klippo_core::store::Store;
use klippo_core::{paths, Source};
use klippo_dbus::{DbusEntry, BUS_NAME, OBJECT_PATH};

/// Concrete clipboard content the UI can publish through GDK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardPayload {
    Text(String),
    Image { mime: String, bytes: Vec<u8> },
}

/// Which Linux selection should receive clipboard content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardTarget {
    Clipboard,
    Primary,
    Both,
}

/// An event pushed from the daemon to the GTK UI.
#[derive(Debug, Clone)]
pub enum UiEvent {
    Show,
    Hide,
    Toggle,
    /// History changed — rebuild the list if visible.
    Refresh,
    /// Copy content to a system clipboard selection (done by the GTK main thread).
    SetClipboard {
        payload: ClipboardPayload,
        target: ClipboardTarget,
    },
    /// Offer an action menu for an entry (entry id + matching action names).
    ActionMenu {
        id: String,
        actions: Vec<String>,
    },
}

/// Shared daemon state.
pub struct AppState {
    store: Mutex<Store>,
    config: RwLock<Config>,
    conn: OnceLock<zbus::Connection>,
    to_ui: OnceLock<async_channel::Sender<UiEvent>>,
    last_heartbeat: Mutex<Option<Instant>>,
}

impl AppState {
    pub fn new(store: Store, config: Config) -> Self {
        Self {
            store: Mutex::new(store),
            config: RwLock::new(config),
            conn: OnceLock::new(),
            to_ui: OnceLock::new(),
            last_heartbeat: Mutex::new(None),
        }
    }

    /// Install the UI event sender (called once, before serving D-Bus).
    pub fn set_ui_sender(&self, tx: async_channel::Sender<UiEvent>) {
        let _ = self.to_ui.set(tx);
    }

    async fn notify_ui(&self, event: UiEvent) {
        if let Some(tx) = self.to_ui.get() {
            let _ = tx.send(event).await;
        }
    }

    /// Notify both external D-Bus subscribers and our own UI that history moved.
    async fn history_changed(&self) {
        if let Some(conn) = self.conn.get() {
            if let Ok(emitter) = SignalEmitter::new(conn, OBJECT_PATH) {
                let _ = Daemon1Iface::history_changed(&emitter).await;
            }
        }
        self.notify_ui(UiEvent::Refresh).await;
    }

    async fn set_clipboard(&self, payload: ClipboardPayload, target: ClipboardTarget) {
        self.notify_ui(UiEvent::SetClipboard { payload, target })
            .await;
    }

    fn payload_from_entry(entry: &Entry) -> Option<ClipboardPayload> {
        match entry.kind {
            EntryKind::Text => entry.text.clone().map(ClipboardPayload::Text),
            EntryKind::Image => {
                let path = entry.image_path.as_ref()?;
                let bytes = std::fs::read(path).ok()?;
                Some(ClipboardPayload::Image {
                    mime: "image/png".to_string(),
                    bytes,
                })
            }
        }
    }

    fn payload_for_entry(&self, id: &str, touch: bool) -> Option<ClipboardPayload> {
        let entry = {
            let store = self.store.lock().ok()?;
            if touch {
                let _ = store.touch(id, now_ms());
            }
            store.get(id).ok().flatten()
        }?;
        Self::payload_from_entry(&entry)
    }

    fn latest_payload(&self) -> Option<ClipboardPayload> {
        let entry = self.store.lock().ok()?.list(1).ok()?.into_iter().next()?;
        Self::payload_from_entry(&entry)
    }

    fn selection_target_for_select(&self) -> ClipboardTarget {
        if self.config.read().unwrap().general.sync_clipboards {
            ClipboardTarget::Both
        } else {
            ClipboardTarget::Clipboard
        }
    }

    async fn maybe_sync_clipboards(&self, entry: &Entry, source: Source) {
        if !self.config.read().unwrap().general.sync_clipboards {
            return;
        }
        let Some(payload) = Self::payload_from_entry(entry) else {
            return;
        };
        let target = match source {
            Source::Clipboard => ClipboardTarget::Primary,
            Source::Primary => ClipboardTarget::Clipboard,
        };
        self.set_clipboard(payload, target).await;
    }

    async fn restore_clipboard_if_needed(&self, source: Source) {
        if source != Source::Clipboard {
            return;
        }
        if !self.config.read().unwrap().general.prevent_empty_clipboard {
            return;
        }
        if let Some(payload) = self.latest_payload() {
            self.set_clipboard(payload, ClipboardTarget::Clipboard)
                .await;
        }
    }

    fn enforce_history_limit(&self, max_items: u32) {
        let removed = self
            .store
            .lock()
            .ok()
            .and_then(|store| store.enforce_limit(max_items).ok())
            .unwrap_or_default();
        for e in &removed {
            gc_entry(e);
        }
    }

    /// Apply config and store a text capture. Returns the stored entry, or
    /// `None` if it was filtered out (ignored selection / empty).
    fn ingest_text(&self, text: &str, source: Source) -> klippo_core::Result<Option<Entry>> {
        let (ignore_selection, strip, max_items) = {
            let c = self.config.read().unwrap();
            (
                c.general.ignore_selection,
                c.general.strip_whitespace,
                c.general.max_items,
            )
        };
        if source == Source::Primary && ignore_selection {
            return Ok(None);
        }
        let processed = if strip { text.trim() } else { text };
        if processed.is_empty() {
            return Ok(None);
        }
        let entry = Entry::new_text(processed, now_ms());
        let pruned = self.store.lock().unwrap().upsert(&entry, max_items)?;
        for e in &pruned {
            gc_entry(e);
        }
        Ok(Some(entry))
    }

    fn config_actions(&self) -> (bool, Vec<klippo_core::actions::Action>) {
        let c = self.config.read().unwrap();
        (c.general.actions_enabled, c.actions.clone())
    }

    /// Run a named action against an entry's text (injection-safe execution).
    async fn run_named_action(&self, id: &str, action_name: &str) -> anyhow::Result<()> {
        let text = self
            .store
            .lock()
            .unwrap()
            .get(id)
            .ok()
            .flatten()
            .and_then(|e| e.text);
        let Some(text) = text else { return Ok(()) };
        let (_, actions) = self.config_actions();
        if let Some(action) = actions.iter().find(|a| a.name == action_name) {
            self.execute_action(&text, action).await?;
        }
        Ok(())
    }

    async fn execute_action(
        &self,
        text: &str,
        action: &klippo_core::actions::Action,
    ) -> anyhow::Result<()> {
        use klippo_core::actions::{match_action, prepare, OutputMode};
        let Some(matched) = match_action(action, text)? else {
            return Ok(());
        };
        for cmd in &action.commands {
            let prepared = prepare(cmd, &matched)?;
            let mut command = tokio::process::Command::new(&prepared.program);
            command.args(&prepared.args);
            match prepared.output {
                OutputMode::Ignore => {
                    // Fire-and-forget (e.g. xdg-open); the child outlives us.
                    command.spawn()?;
                }
                OutputMode::ReplaceClipboard => {
                    let out = command.output().await?;
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() {
                        self.set_clipboard(
                            ClipboardPayload::Text(s.clone()),
                            ClipboardTarget::Clipboard,
                        )
                        .await;
                        let _ = self.ingest_text(&s, Source::Clipboard);
                        self.history_changed().await;
                    }
                }
                OutputMode::NewEntry => {
                    let out = command.output().await?;
                    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    if !s.is_empty() {
                        let _ = self.ingest_text(&s, Source::Clipboard);
                        self.history_changed().await;
                    }
                }
            }
        }
        Ok(())
    }

    /// If actions are enabled and an `automatic` action matches, ask the UI to
    /// pop up an action menu for this entry.
    async fn maybe_emit_action_popup(&self, entry: &Entry) {
        let Some(text) = entry.text.as_deref() else {
            return;
        };
        let (enabled, actions) = self.config_actions();
        if !enabled {
            return;
        }
        let offered: Vec<(String, String)> = actions
            .iter()
            .filter(|a| a.automatic)
            .filter(|a| matches!(klippo_core::actions::match_action(a, text), Ok(Some(_))))
            .map(|a| {
                let label = a
                    .commands
                    .first()
                    .map(|c| c.command.clone())
                    .unwrap_or_default();
                (a.name.clone(), label)
            })
            .collect();
        if offered.is_empty() {
            return;
        }
        let names: Vec<String> = offered.iter().map(|(n, _)| n.clone()).collect();
        if let Some(conn) = self.conn.get() {
            if let Ok(emitter) = SignalEmitter::new(conn, OBJECT_PATH) {
                let _ = Daemon1Iface::action_popup_requested(&emitter, &entry.id, offered).await;
            }
        }
        self.notify_ui(UiEvent::ActionMenu {
            id: entry.id.clone(),
            actions: names,
        })
        .await;
    }

    // --- UI-facing synchronous helpers (called on the GTK main thread) ---

    pub(crate) fn entries(&self) -> Vec<Entry> {
        let max = self.config.read().unwrap().general.max_items;
        self.store.lock().unwrap().list(max).unwrap_or_default()
    }

    /// Promote an entry to the top and return its content (to copy to clipboard).
    pub(crate) fn ui_select(&self, id: &str) -> Option<(ClipboardPayload, ClipboardTarget)> {
        let payload = self.payload_for_entry(id, true)?;
        Some((payload, self.selection_target_for_select()))
    }

    pub(crate) fn ui_remove(&self, id: &str) {
        let removed = self.store.lock().unwrap().remove(id).ok().flatten();
        if let Some(e) = removed {
            gc_entry(&e);
        }
    }

    pub(crate) fn ui_clear(&self) {
        let removed = self.store.lock().unwrap().clear().unwrap_or_default();
        for e in &removed {
            gc_entry(e);
        }
    }

    pub(crate) fn popup_width(&self) -> u32 {
        self.config.read().unwrap().ui.popup_width
    }

    pub(crate) fn popup_max_rows(&self) -> u32 {
        self.config.read().unwrap().ui.popup_max_rows
    }

    pub(crate) fn color_scheme(&self) -> ColorScheme {
        self.config.read().unwrap().ui.color_scheme
    }

    /// A clone of the current config (for the Settings dialog).
    pub(crate) fn config_snapshot(&self) -> Config {
        self.config.read().unwrap().clone()
    }

    /// Mutate + persist the config (used by the Settings dialog).
    pub(crate) fn update_config(&self, edit: impl FnOnce(&mut Config)) {
        let max_items = {
            let mut c = self.config.write().unwrap();
            edit(&mut c);
            let _ = c.save();
            c.general.max_items
        };
        self.enforce_history_limit(max_items);
    }

    /// Full text of an entry (for the Edit and QR dialogs).
    pub(crate) fn entry_content(&self, id: &str) -> Option<String> {
        self.store
            .lock()
            .unwrap()
            .get(id)
            .ok()
            .flatten()
            .and_then(|e| e.text)
    }

    /// Replace an entry's text: store the edit as a new top entry and drop the
    /// original. Returns the new id (or `None` if the text was empty).
    pub(crate) fn replace_entry_text(&self, old_id: &str, new_text: &str) -> Option<String> {
        if new_text.is_empty() {
            return None;
        }
        let max = self.config.read().unwrap().general.max_items;
        let entry = Entry::new_text(new_text, now_ms());
        let store = self.store.lock().unwrap();
        if let Ok(pruned) = store.upsert(&entry, max) {
            for e in &pruned {
                gc_entry(e);
            }
        }
        if old_id != entry.id {
            if let Ok(Some(old)) = store.remove(old_id) {
                gc_entry(&old);
            }
        }
        Some(entry.id)
    }

    /// Names of configured actions whose regex matches an entry's text.
    pub(crate) fn matching_action_names(&self, id: &str) -> Vec<String> {
        let Some(text) = self.entry_content(id) else {
            return Vec::new();
        };
        let (enabled, actions) = self.config_actions();
        if !enabled {
            return Vec::new();
        }
        actions
            .iter()
            .filter(|a| matches!(klippo_core::actions::match_action(a, &text), Ok(Some(_))))
            .map(|a| a.name.clone())
            .collect()
    }

    /// Synchronous action execution for the UI. Returns text to put on the
    /// clipboard when a command uses `ReplaceClipboard` output.
    pub(crate) fn run_action_blocking(&self, id: &str, action_name: &str) -> Option<String> {
        use klippo_core::actions::{match_action, prepare, OutputMode};
        let text = self.entry_content(id)?;
        let (_, actions) = self.config_actions();
        let action = actions.iter().find(|a| a.name == action_name)?;
        let matched = match match_action(action, &text) {
            Ok(Some(m)) => m,
            _ => return None,
        };
        let mut clipboard_out = None;
        for cmd in &action.commands {
            let Ok(prepared) = prepare(cmd, &matched) else {
                continue;
            };
            let mut command = std::process::Command::new(&prepared.program);
            command.args(&prepared.args);
            match prepared.output {
                OutputMode::Ignore => {
                    let _ = command.spawn();
                }
                OutputMode::ReplaceClipboard => {
                    if let Ok(out) = command.output() {
                        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if !s.is_empty() {
                            let _ = self.ingest_text(&s, Source::Clipboard);
                            clipboard_out = Some(s);
                        }
                    }
                }
                OutputMode::NewEntry => {
                    if let Ok(out) = command.output() {
                        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                        if !s.is_empty() {
                            let _ = self.ingest_text(&s, Source::Clipboard);
                        }
                    }
                }
            }
        }
        clipboard_out
    }

    /// Store an image capture (PNG bytes) with a thumbnail. `None` if filtered
    /// out by config (ignore images / selection text only).
    fn ingest_image(
        &self,
        _mime: &str,
        bytes: &[u8],
        source: Source,
    ) -> anyhow::Result<Option<Entry>> {
        let (ignore_images, selection_text_only, max_items) = {
            let c = self.config.read().unwrap();
            (
                c.general.ignore_images,
                c.general.selection_text_only,
                c.general.max_items,
            )
        };
        if ignore_images || (source == Source::Primary && selection_text_only) {
            return Ok(None);
        }

        let id = klippo_core::model::hash_bytes(bytes);
        {
            let store = self.store.lock().unwrap();
            if let Some(existing) = store.get(&id)? {
                let _ = store.touch(&id, now_ms());
                return Ok(Some(existing));
            }
        }

        let img = image::load_from_memory(bytes)?;
        let (w, h) = (img.width(), img.height());
        let images_dir = paths::images_dir();
        let thumbs_dir = paths::thumbs_dir();
        std::fs::create_dir_all(&images_dir)?;
        std::fs::create_dir_all(&thumbs_dir)?;
        let img_path = images_dir.join(format!("{id}.png"));
        let thumb_path = thumbs_dir.join(format!("{id}.png"));
        std::fs::write(&img_path, bytes)?;
        img.thumbnail(96, 64).save(&thumb_path)?;
        let entry = Entry::new_image(id, img_path, thumb_path, w, h, now_ms());
        let pruned = self.store.lock().unwrap().upsert(&entry, max_items)?;
        for e in &pruned {
            gc_entry(e);
        }
        Ok(Some(entry))
    }

    // --- config get/set used over D-Bus (e.g. by the GNOME extension) ---

    fn get_config_str(&self, key: &str) -> String {
        let c = self.config.read().unwrap();
        match key {
            "ignore_images" => c.general.ignore_images.to_string(),
            "ignore_selection" => c.general.ignore_selection.to_string(),
            "keep_clipboard_contents" => c.general.keep_clipboard_contents.to_string(),
            "selection_text_only" => c.general.selection_text_only.to_string(),
            "sync_clipboards" => c.general.sync_clipboards.to_string(),
            "prevent_empty_clipboard" => c.general.prevent_empty_clipboard.to_string(),
            "actions_enabled" => c.general.actions_enabled.to_string(),
            "max_items" => c.general.max_items.to_string(),
            _ => String::new(),
        }
    }

    fn set_config_str(&self, key: &str, value: &str) -> klippo_core::Result<()> {
        let max_items = {
            let mut c = self.config.write().unwrap();
            match key {
                "ignore_images" => {
                    c.general.ignore_images = value.parse().unwrap_or(c.general.ignore_images)
                }
                "ignore_selection" => {
                    c.general.ignore_selection = value.parse().unwrap_or(c.general.ignore_selection)
                }
                "keep_clipboard_contents" => {
                    c.general.keep_clipboard_contents =
                        value.parse().unwrap_or(c.general.keep_clipboard_contents)
                }
                "selection_text_only" => {
                    c.general.selection_text_only =
                        value.parse().unwrap_or(c.general.selection_text_only)
                }
                "sync_clipboards" => {
                    c.general.sync_clipboards = value.parse().unwrap_or(c.general.sync_clipboards)
                }
                "prevent_empty_clipboard" => {
                    c.general.prevent_empty_clipboard =
                        value.parse().unwrap_or(c.general.prevent_empty_clipboard)
                }
                "actions_enabled" => {
                    c.general.actions_enabled = value.parse().unwrap_or(c.general.actions_enabled)
                }
                "max_items" => c.general.max_items = value.parse().unwrap_or(c.general.max_items),
                _ => {}
            }
            c.save()?;
            c.general.max_items
        };
        self.enforce_history_limit(max_items);
        Ok(())
    }
}

fn gc_entry(e: &Entry) {
    for p in [e.image_path.as_ref(), e.thumb_path.as_ref()]
        .into_iter()
        .flatten()
    {
        let _ = std::fs::remove_file(p);
    }
}

fn to_dbus_entry(e: Entry) -> DbusEntry {
    DbusEntry {
        id: e.id,
        kind: match e.kind {
            EntryKind::Text => "text",
            EntryKind::Image => "image",
        }
        .to_string(),
        preview: e.preview,
        thumb_path: e
            .thumb_path
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        timestamp_ms: e.timestamp_ms,
        pinned: e.pinned,
    }
}

fn to_fdo<E: std::fmt::Display>(e: E) -> fdo::Error {
    fdo::Error::Failed(e.to_string())
}

/// Control + query interface.
pub struct Daemon1Iface {
    pub state: Arc<AppState>,
}

#[interface(name = "org.klippo.Daemon1")]
impl Daemon1Iface {
    async fn toggle(&self) {
        self.state.notify_ui(UiEvent::Toggle).await;
    }

    async fn show(&self) {
        self.state.notify_ui(UiEvent::Show).await;
    }

    async fn hide(&self) {
        self.state.notify_ui(UiEvent::Hide).await;
    }

    async fn clear(&self) -> fdo::Result<()> {
        self.state.ui_clear();
        self.state.history_changed().await;
        Ok(())
    }

    async fn select(&self, id: &str) -> fdo::Result<()> {
        if let Some((payload, target)) = self.state.ui_select(id) {
            self.state.set_clipboard(payload, target).await;
        }
        self.state.notify_ui(UiEvent::Hide).await;
        Ok(())
    }

    async fn remove_entry(&self, id: &str) -> fdo::Result<()> {
        self.state.ui_remove(id);
        self.state.history_changed().await;
        Ok(())
    }

    async fn run_action(&self, id: &str, action_name: &str) -> fdo::Result<()> {
        self.state
            .run_named_action(id, action_name)
            .await
            .map_err(to_fdo)
    }

    async fn list_entries(&self, limit: u32) -> fdo::Result<Vec<DbusEntry>> {
        let entries = {
            let store = self.state.store.lock().unwrap();
            store.list(limit).map_err(to_fdo)?
        };
        Ok(entries.into_iter().map(to_dbus_entry).collect())
    }

    async fn get_entry_content(&self, id: &str) -> fdo::Result<(String, Vec<u8>)> {
        match self.state.payload_for_entry(id, false) {
            Some(ClipboardPayload::Text(text)) => {
                Ok(("text/plain;charset=utf-8".to_string(), text.into_bytes()))
            }
            Some(ClipboardPayload::Image { mime, bytes }) => Ok((mime, bytes)),
            None => Err(fdo::Error::Failed(format!("no entry with id {id}"))),
        }
    }

    async fn get_config(&self, key: &str) -> String {
        self.state.get_config_str(key)
    }

    async fn set_config(&self, key: &str, value: &str) -> fdo::Result<()> {
        self.state.set_config_str(key, value).map_err(to_fdo)?;
        if let Some(conn) = self.state.conn.get() {
            if let Ok(emitter) = SignalEmitter::new(conn, OBJECT_PATH) {
                let _ = Daemon1Iface::config_changed(&emitter, key).await;
            }
        }
        Ok(())
    }

    async fn ping(&self) -> String {
        format!("klippo {}", env!("CARGO_PKG_VERSION"))
    }

    #[zbus(signal)]
    async fn history_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn config_changed(emitter: &SignalEmitter<'_>, key: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_popup_requested(
        emitter: &SignalEmitter<'_>,
        id: &str,
        actions: Vec<(String, String)>,
    ) -> zbus::Result<()>;
}

/// Capture push interface (called by the GNOME Shell extension).
pub struct Capture1Iface {
    pub state: Arc<AppState>,
}

#[interface(name = "org.klippo.Capture1")]
impl Capture1Iface {
    async fn add_text(&self, text: &str, source: &str) -> fdo::Result<()> {
        let source = parse_source(source);
        if let Some(entry) = self.state.ingest_text(text, source).map_err(to_fdo)? {
            self.state.history_changed().await;
            self.state.maybe_sync_clipboards(&entry, source).await;
            self.state.maybe_emit_action_popup(&entry).await;
        }
        Ok(())
    }

    async fn add_image(&self, mime: &str, bytes: Vec<u8>, source: &str) -> fdo::Result<()> {
        let source = parse_source(source);
        if let Some(entry) = self
            .state
            .ingest_image(mime, &bytes, source)
            .map_err(to_fdo)?
        {
            self.state.history_changed().await;
            self.state.maybe_sync_clipboards(&entry, source).await;
        }
        Ok(())
    }

    async fn clipboard_cleared(&self, source: &str) {
        self.state
            .restore_clipboard_if_needed(parse_source(source))
            .await;
    }

    async fn heartbeat(&self) {
        *self.state.last_heartbeat.lock().unwrap() = Some(Instant::now());
    }
}

/// Build the zbus connection, serve both interfaces, and keep them alive.
async fn serve_dbus(state: Arc<AppState>) -> anyhow::Result<()> {
    let conn = zbus::connection::Builder::session()?
        .name(BUS_NAME)?
        .serve_at(
            OBJECT_PATH,
            Daemon1Iface {
                state: state.clone(),
            },
        )?
        .serve_at(
            OBJECT_PATH,
            Capture1Iface {
                state: state.clone(),
            },
        )?
        .build()
        .await?;
    let _ = state.conn.set(conn);
    info!(bus = BUS_NAME, path = OBJECT_PATH, "D-Bus service ready");

    spawn_capture(state.clone());

    // Keep the task (and thus the served connection) alive.
    std::future::pending::<()>().await;
    Ok(())
}

/// Start the capture backend appropriate for this session.
///
/// On GNOME Wayland events arrive via the Shell extension (D-Bus), so no source
/// is started. X11 forwards `ClipboardEvent`s through a channel into [`consume`];
/// the Wayland data-control source self-feeds via `klippo __feed`.
fn spawn_capture(state: Arc<AppState>) {
    use klippo_capture::{ClipboardEvent, ClipboardSource, WaylandDataControlSource, X11Source};

    match detect_backend() {
        BackendKind::GnomeBridge => {
            info!("backend: GNOME bridge — clipboard events arrive via the Shell extension");
        }
        BackendKind::X11 => {
            info!("backend: X11 (polling)");
            let (tx, rx) = tokio::sync::mpsc::channel::<ClipboardEvent>(32);
            tokio::spawn(async move {
                if let Err(e) = Box::new(X11Source::new()).run(tx).await {
                    warn!(error = %e, "X11 capture source ended");
                }
            });
            tokio::spawn(consume(rx, state));
        }
        BackendKind::WaylandDataControl => {
            info!("backend: Wayland data-control (wl-paste)");
            let (tx, _rx) = tokio::sync::mpsc::channel::<ClipboardEvent>(1);
            tokio::spawn(async move {
                if let Err(e) = Box::new(WaylandDataControlSource::new()).run(tx).await {
                    warn!(error = %e, "Wayland data-control source failed to start");
                }
            });
        }
        BackendKind::None => warn!("no usable capture backend detected"),
    }
}

/// Drain `ClipboardEvent`s from a source into the store, emitting changes.
async fn consume(
    mut rx: tokio::sync::mpsc::Receiver<klippo_capture::ClipboardEvent>,
    state: Arc<AppState>,
) {
    use klippo_capture::ClipboardEvent;
    while let Some(event) = rx.recv().await {
        match event {
            ClipboardEvent::Text { text, source } => {
                if let Ok(Some(entry)) = state.ingest_text(&text, source) {
                    state.history_changed().await;
                    state.maybe_sync_clipboards(&entry, source).await;
                    state.maybe_emit_action_popup(&entry).await;
                }
            }
            ClipboardEvent::Image {
                mime,
                bytes,
                source,
            } => {
                if let Some(entry) = state.ingest_image(&mime, &bytes, source).ok().flatten() {
                    state.history_changed().await;
                    state.maybe_sync_clipboards(&entry, source).await;
                }
            }
            ClipboardEvent::Cleared { source } => state.restore_clipboard_if_needed(source).await,
        }
    }
}

/// Entry point for `klippo daemon`: set up state + the background runtime, then
/// hand the main thread to GTK.
pub fn run() -> anyhow::Result<()> {
    let config = Config::load()?;
    let store = Store::open(&paths::db_path())?;
    if !config.general.keep_clipboard_contents {
        let removed = store.clear()?;
        for e in &removed {
            gc_entry(e);
        }
    }
    info!(db = %paths::db_path().display(), "history store opened");

    let (to_ui_tx, to_ui_rx) = async_channel::unbounded::<UiEvent>();
    let state = Arc::new(AppState::new(store, config));
    state.set_ui_sender(to_ui_tx);

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let dbus_state = state.clone();
    rt.spawn(async move {
        if let Err(e) = serve_dbus(dbus_state).await {
            error!(error = %e, "D-Bus service failed");
        }
    });

    // Runs the GTK main loop; keeps `rt` alive for its duration.
    crate::ui::run(state, to_ui_rx, rt);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use klippo_core::model::hash_bytes;

    fn state() -> AppState {
        AppState::new(Store::open_in_memory().unwrap(), Config::default())
    }

    #[test]
    fn ui_select_returns_text_payload_and_promotes() {
        let state = state();
        let a = Entry::new_text("old", 1);
        let b = Entry::new_text("new", 2);
        state.store.lock().unwrap().upsert(&a, 25).unwrap();
        state.store.lock().unwrap().upsert(&b, 25).unwrap();

        let selected = state.ui_select(&a.id).unwrap();

        assert_eq!(
            selected,
            (
                ClipboardPayload::Text("old".to_string()),
                ClipboardTarget::Clipboard
            )
        );
        let list = state.entries();
        assert_eq!(list[0].text.as_deref(), Some("old"));
    }

    #[test]
    fn ui_select_returns_image_payload() {
        let state = state();
        let path = std::env::temp_dir().join(format!(
            "klippo-test-image-{}-{}.png",
            std::process::id(),
            now_ms()
        ));
        let bytes = tiny_png();
        std::fs::write(&path, &bytes).unwrap();
        let entry = Entry::new_image(hash_bytes(&bytes), path.clone(), path.clone(), 1, 1, 1);
        state.store.lock().unwrap().upsert(&entry, 25).unwrap();

        let selected = state.ui_select(&entry.id).unwrap();

        assert_eq!(
            selected,
            (
                ClipboardPayload::Image {
                    mime: "image/png".to_string(),
                    bytes: bytes.clone()
                },
                ClipboardTarget::Clipboard
            )
        );
        let _ = std::fs::remove_file(path);
    }

    fn tiny_png() -> Vec<u8> {
        let mut bytes = Vec::new();
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]));
        let dyn_img = image::DynamicImage::ImageRgba8(img);
        dyn_img
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }
}
