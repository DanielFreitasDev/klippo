//! The GTK4 + libadwaita popup UI, hosted inside the daemon process.
//!
//! Runs as a background `adw::Application` (held alive with `hold()`) whose
//! window is shown/hidden on demand. The window mimics Klipper: a search box on
//! top, a most-recent-first list, hover-reveal per-row buttons (run actions /
//! show QR / edit / delete), a Clear-all and a Settings button, plus dialogs for
//! settings, editing an item, and showing a QR code.
//!
//! Daemon → UI events arrive on an `async-channel` awaited on the GTK main loop;
//! UI → state calls go straight to [`AppState`] (synchronous SQLite).

use std::rc::Rc;
use std::sync::Arc;

use gtk4 as gtk;
use gtk4::gdk_pixbuf;
use gtk4::gdk_pixbuf::prelude::*;
use gtk4::prelude::*;
use gtk4::{gdk, glib, pango};
use libadwaita as adw;
use libadwaita::prelude::*;

use klippo_core::config::ColorScheme;
use klippo_core::model::EntryKind;
use klippo_core::search;

use crate::daemon::{AppState, ClipboardPayload, ClipboardTarget, UiEvent};

const STYLE: &str = include_str!("../../../data/style.css");

/// Shared UI handles, cloned (cheaply, via `Rc`) into widget callbacks.
struct Ui {
    state: Arc<AppState>,
    window: adw::ApplicationWindow,
    search: gtk::SearchEntry,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    /// Warns when the GNOME capture extension stops sending heartbeats.
    banner: adw::Banner,
    /// Whether the window has been focused since the last show (so we don't
    /// auto-hide a window that opened unfocused).
    was_active: std::cell::Cell<bool>,
    /// Whether a child dialog (settings/edit/QR) is open — suppresses auto-hide.
    dialog_open: std::cell::Cell<bool>,
}

/// Build the application, wire it up, and run the GTK main loop (blocking).
///
/// `_rt` (the Tokio runtime serving D-Bus) is kept alive for the loop's
/// duration and dropped when the app exits.
pub fn run(
    state: Arc<AppState>,
    rx: async_channel::Receiver<UiEvent>,
    _rt: tokio::runtime::Runtime,
) {
    let app = adw::Application::builder()
        .application_id("org.klippo")
        .build();

    app.connect_startup(move |app| {
        load_css(&state.font_family());
        apply_color_scheme(state.color_scheme());
        build_window(app, state.clone(), rx.clone());
    });

    // The window is shown on demand (via D-Bus), not at activation.
    app.connect_activate(|_| {});

    // Pass no args: our CLI subcommand ("daemon") must not reach GApplication.
    app.run_with_args::<&str>(&[]);
}

thread_local! {
    /// Provider holding the user-configured font, kept on the main thread so the
    /// Settings dialog can hot-swap the family without rebuilding the window.
    static FONT_PROVIDER: gtk::CssProvider = gtk::CssProvider::new();
}

/// Load the bundled base stylesheet plus a provider for the configured font.
fn load_css(font_family: &str) {
    let base = gtk::CssProvider::new();
    base.load_from_data(STYLE);
    FONT_PROVIDER.with(|fp| fp.load_from_data(&font_css(font_family)));
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &base,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        // One step above the base so the configured font wins.
        FONT_PROVIDER.with(|fp| {
            gtk::style_context_add_provider_for_display(
                &display,
                fp,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            )
        });
    }
}

/// Hot-swap the configured font family (called from the Settings dialog).
fn apply_font(font_family: &str) {
    FONT_PROVIDER.with(|fp| fp.load_from_data(&font_css(font_family)));
}

/// CSS overriding the monospace previews/index with `font_family`. Quotes are
/// stripped so a stray `"` in the value can't break out of the declaration.
fn font_css(font_family: &str) -> String {
    format!(
        ".klippo-preview, .klippo-index {{ font-family: \"{}\", monospace; }}",
        font_family.replace('"', "")
    )
}

fn apply_color_scheme(scheme: ColorScheme) {
    let manager = adw::StyleManager::default();
    manager.set_color_scheme(match scheme {
        ColorScheme::System => adw::ColorScheme::Default,
        ColorScheme::Light => adw::ColorScheme::ForceLight,
        ColorScheme::Dark => adw::ColorScheme::ForceDark,
    });
}

fn build_window(
    app: &adw::Application,
    state: Arc<AppState>,
    rx: async_channel::Receiver<UiEvent>,
) {
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(state.popup_width() as i32)
        .resizable(true)
        .title("Klippo")
        .build();
    window.add_css_class("klippo");

    // Keep the service alive even with no window visible; the guard lives in the
    // event loop below (dropped only when the app exits).
    let hold = app.hold();

    let search = gtk::SearchEntry::builder()
        .placeholder_text("Pesquisar…")
        .build();
    search.add_css_class("klippo-search");

    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("klippo-list");

    // Size the popup to its content (Klipper-style), growing only up to a cap
    // and then scrolling — so a few items make a small window, not a tall one.
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&list)
        .build();

    let settings_btn = gtk::Button::builder()
        .icon_name("emblem-system-symbolic")
        .tooltip_text("Configurações")
        .build();
    settings_btn.add_css_class("flat");
    let clear_btn = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .tooltip_text("Limpar tudo")
        .build();
    clear_btn.add_css_class("flat");

    let footer = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    footer.set_margin_start(4);
    footer.set_margin_end(4);
    footer.set_margin_bottom(4);
    footer.append(&gtk::Box::builder().hexpand(true).build()); // spacer
    footer.append(&settings_btn);
    footer.append(&clear_btn);

    // Shown only when the GNOME capture extension stops sending heartbeats.
    let banner = adw::Banner::builder()
        .title("Extensão do Klippo inativa — rode 'klippo setup' e refaça login.")
        .revealed(false)
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    content.append(&banner);
    content.append(&search);
    content.append(&scroller);
    content.append(&footer);
    window.set_content(Some(&content));

    let ui = Rc::new(Ui {
        state,
        window: window.clone(),
        search: search.clone(),
        list: list.clone(),
        scroller: scroller.clone(),
        banner: banner.clone(),
        was_active: std::cell::Cell::new(false),
        dialog_open: std::cell::Cell::new(false),
    });

    ui.rebuild("");

    // Live filter; clearing restores the full list.
    {
        let ui = ui.clone();
        search.connect_search_changed(move |entry| ui.rebuild(entry.text().as_str()));
    }
    // Enter selects the top result.
    {
        let ui = ui.clone();
        search.connect_activate(move |_| {
            if let Some(row) = ui.list.row_at_index(0) {
                let id = row.widget_name().to_string();
                if !id.is_empty() {
                    ui.select(&id);
                }
            }
        });
    }
    // Click / Enter on a row selects it.
    {
        let ui = ui.clone();
        list.connect_row_activated(move |_, row| {
            let id = row.widget_name().to_string();
            if !id.is_empty() {
                ui.select(&id);
            }
        });
    }
    // Settings.
    {
        let ui = ui.clone();
        settings_btn.connect_clicked(move |_| ui.open_settings());
    }
    // Clear all.
    {
        let ui = ui.clone();
        clear_btn.connect_clicked(move |_| {
            ui.state.ui_clear();
            ui.rebuild(ui.search.text().as_str());
        });
    }
    // Esc hides; Alt+1..9 selects the Nth visible item (Klipper-style).
    {
        let key = gtk::EventControllerKey::new();
        let ui = ui.clone();
        key.connect_key_pressed(move |_, keyval, _, state| {
            if keyval == gdk::Key::Escape {
                ui.window.set_visible(false);
                return glib::Propagation::Stop;
            }
            // Down from the search box moves into the list; arrows then navigate it.
            if keyval == gdk::Key::Down && ui.search.has_focus() {
                if let Some(row) = ui.list.row_at_index(0) {
                    ui.list.select_row(Some(&row));
                    row.grab_focus();
                    return glib::Propagation::Stop;
                }
            }
            if state.contains(gdk::ModifierType::ALT_MASK) {
                if let Some(n) = keyval.to_unicode().and_then(|c| c.to_digit(10)) {
                    if n >= 1 {
                        if let Some(row) = ui.list.row_at_index(n as i32 - 1) {
                            let id = row.widget_name().to_string();
                            if !id.is_empty() {
                                ui.select(&id);
                                return glib::Propagation::Stop;
                            }
                        }
                    }
                }
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key);
    }
    // Closing only hides (daemon keeps running).
    window.connect_close_request(|win| {
        win.set_visible(false);
        glib::Propagation::Stop
    });

    // Auto-hide when the popup loses focus (click outside), like Klipper.
    // Guarded so it doesn't fire while a child dialog is open, and only after
    // the window has actually been focused (a window that opens unfocused on
    // Wayland won't immediately vanish).
    {
        let ui = ui.clone();
        window.connect_is_active_notify(move |win| {
            if win.is_active() {
                ui.was_active.set(true);
            } else if ui.was_active.get() && !ui.dialog_open.get() {
                ui.was_active.set(false);
                win.set_visible(false);
            }
        });
    }

    // Daemon → UI event loop.
    {
        let ui = ui.clone();
        glib::spawn_future_local(async move {
            let _hold = hold; // keep the app alive for as long as the loop runs
            while let Ok(event) = rx.recv().await {
                match event {
                    UiEvent::Show => {
                        ui.rebuild(ui.search.text().as_str());
                        ui.present();
                    }
                    UiEvent::Hide => ui.window.set_visible(false),
                    UiEvent::Toggle => {
                        if ui.window.is_visible() {
                            ui.window.set_visible(false);
                        } else {
                            ui.search.set_text("");
                            ui.rebuild("");
                            ui.present();
                        }
                    }
                    UiEvent::Refresh => {
                        if ui.window.is_visible() {
                            ui.rebuild(ui.search.text().as_str());
                        }
                    }
                    UiEvent::ConfigReloaded => {
                        apply_color_scheme(ui.state.color_scheme());
                        apply_font(&ui.state.font_family());
                        if ui.window.is_visible() {
                            ui.rebuild(ui.search.text().as_str());
                        }
                    }
                    UiEvent::SetClipboard { payload, target } => ui.set_clipboard(&payload, target),
                    UiEvent::ActionMenu { id, actions } => {
                        if !ui.window.is_visible() {
                            ui.present();
                        }
                        ui.show_action_menu(&ui.search, &id, actions);
                    }
                }
            }
        });
    }
}

impl Ui {
    fn present(&self) {
        self.was_active.set(false); // re-arm focus-loss auto-hide for this show
        self.update_extension_banner();
        self.window.present();
        self.search.grab_focus();
    }

    /// Reveal the warning banner only when the GNOME extension is the active
    /// backend and has gone silent (no recent heartbeats).
    fn update_extension_banner(&self) {
        let stale = self.state.extension_status() == Some(false);
        self.banner.set_revealed(stale);
    }

    fn select(self: &Rc<Self>, id: &str) {
        if let Some((payload, target)) = self.state.ui_select(id) {
            self.set_clipboard(&payload, target);
        }
        // Re-run an action on re-select when ReplayActionInHistory is on.
        if let Some(replaced) = self.state.maybe_replay_action_blocking(id) {
            self.window.clipboard().set_text(&replaced);
        }
        self.window.set_visible(false);
    }

    fn set_clipboard(&self, payload: &ClipboardPayload, target: ClipboardTarget) {
        match target {
            ClipboardTarget::Clipboard => self.set_one_clipboard(&self.window.clipboard(), payload),
            ClipboardTarget::Primary => {
                self.set_one_clipboard(&self.window.primary_clipboard(), payload)
            }
            ClipboardTarget::Both => {
                self.set_one_clipboard(&self.window.clipboard(), payload);
                self.set_one_clipboard(&self.window.primary_clipboard(), payload);
            }
        }
    }

    fn set_one_clipboard(&self, clipboard: &gdk::Clipboard, payload: &ClipboardPayload) {
        match payload {
            ClipboardPayload::Text(text) => clipboard.set_text(text),
            ClipboardPayload::Image { bytes, .. } => {
                let loader = gdk_pixbuf::PixbufLoader::new();
                if let Err(e) = loader.write(bytes).and_then(|_| loader.close()) {
                    tracing::warn!(error = %e, "could not decode image for clipboard");
                    return;
                }
                if let Some(pixbuf) = loader.pixbuf() {
                    let texture = gdk::Texture::for_pixbuf(&pixbuf);
                    clipboard.set_texture(&texture);
                }
            }
        }
    }

    /// Rebuild the list from the store, filtered by `query`.
    fn rebuild(self: &Rc<Self>, query: &str) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }

        let entries = self.state.entries();
        let filtered = search::filter(&entries, query);

        if filtered.is_empty() {
            let label = gtk::Label::new(Some(if entries.is_empty() {
                "Histórico vazio"
            } else {
                "Nenhum resultado"
            }));
            label.add_css_class("klippo-empty");
            let row = gtk::ListBoxRow::new();
            row.set_selectable(false);
            row.set_activatable(false);
            row.set_child(Some(&label));
            self.list.append(&row);
            self.fit_height(0);
            return;
        }

        let count = filtered.len();
        for (index, entry) in filtered.into_iter().enumerate() {
            self.list.append(&self.make_row(index, entry));
        }
        self.fit_height(count);
    }

    /// Resize the scrolled area to fit `rows` items (capped at `popup_max_rows`)
    /// so the popup is compact for a few items and scrolls for many.
    fn fit_height(&self, rows: usize) {
        const ROW_PX: i32 = 38;
        let max_rows = self.state.popup_max_rows().max(1) as usize;
        let shown = rows.clamp(1, max_rows) as i32;
        let h = if rows == 0 { 64 } else { shown * ROW_PX };
        self.scroller.set_min_content_height(h);
        self.scroller.set_max_content_height(h);
    }

    fn make_row(self: &Rc<Self>, index: usize, entry: &klippo_core::Entry) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_widget_name(&entry.id); // stash id for activation/buttons
        row.set_activatable(true);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 6);

        // Klipper-style index for the first 9 items (Alt+N selects it).
        let num_text = if index < 9 {
            format!("{}", index + 1)
        } else {
            String::new()
        };
        let num = gtk::Label::new(Some(&num_text));
        num.add_css_class("klippo-index");
        num.set_width_chars(2);
        hbox.append(&num);

        // Leading: thumbnail for images, otherwise the text preview.
        if entry.kind == EntryKind::Image {
            if let Some(path) = &entry.thumb_path {
                let pic = gtk::Picture::for_filename(path);
                pic.set_size_request(96, 48);
                pic.set_halign(gtk::Align::Start);
                hbox.append(&pic);
            }
        }
        let label = gtk::Label::new(Some(&entry.preview));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        label.set_ellipsize(pango::EllipsizeMode::End);
        label.add_css_class("klippo-preview");
        hbox.append(&label);

        // Pin/unpin: pinned rows sort above the MRU order and keep the pin lit.
        let pin_btn = row_button(
            "view-pin-symbolic",
            if entry.pinned { "Desafixar" } else { "Fixar" },
        );
        if entry.pinned {
            pin_btn.add_css_class("klippo-pinned");
        }
        {
            let (ui, id, pinned) = (self.clone(), entry.id.clone(), entry.pinned);
            pin_btn.connect_clicked(move |_| {
                ui.state.ui_set_pinned(&id, !pinned);
                ui.rebuild(ui.search.text().as_str());
            });
        }
        hbox.append(&pin_btn);

        // Trailing hover-reveal buttons: [pin] [actions?] [QR] [Edit] [Delete].
        if !self.state.matching_action_names(&entry.id).is_empty() {
            let actions_btn = row_button("system-run-symbolic", "Executar ação");
            let (ui, id) = (self.clone(), entry.id.clone());
            actions_btn.connect_clicked(move |btn| {
                let names = ui.state.matching_action_names(&id);
                ui.show_action_menu(btn, &id, names);
            });
            hbox.append(&actions_btn);
        }

        // QR only makes sense for (short-ish) text.
        if entry.kind == EntryKind::Text {
            let qr_btn = row_button("view-barcode-qr-symbolic", "Mostrar QR code");
            let (ui, id) = (self.clone(), entry.id.clone());
            qr_btn.connect_clicked(move |_| ui.open_qr(&id));
            hbox.append(&qr_btn);

            let edit_btn = row_button("document-edit-symbolic", "Editar conteúdo");
            let (ui, id) = (self.clone(), entry.id.clone());
            edit_btn.connect_clicked(move |_| ui.open_edit(&id));
            hbox.append(&edit_btn);
        }

        let delete_btn = row_button("window-close-symbolic", "Remover do histórico");
        {
            let (ui, id) = (self.clone(), entry.id.clone());
            delete_btn.connect_clicked(move |_| {
                ui.state.ui_remove(&id);
                ui.rebuild(ui.search.text().as_str());
            });
        }
        hbox.append(&delete_btn);

        row.set_child(Some(&hbox));
        row
    }

    fn show_action_menu(
        self: &Rc<Self>,
        anchor: &impl IsA<gtk::Widget>,
        id: &str,
        names: Vec<String>,
    ) {
        if names.is_empty() {
            return;
        }
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        for name in names {
            let btn = gtk::Button::with_label(&name);
            btn.add_css_class("flat");
            btn.set_halign(gtk::Align::Fill);
            let (ui, id, pop, name) = (self.clone(), id.to_string(), popover.clone(), name.clone());
            btn.connect_clicked(move |_| {
                if let Some(text) = ui.state.run_action_blocking(&id, &name) {
                    ui.window.clipboard().set_text(&text);
                }
                pop.popdown();
                ui.window.set_visible(false);
            });
            vbox.append(&btn);
        }
        popover.set_child(Some(&vbox));
        popover.popup();

        // Auto-dismiss after the configured timeout (0 = stay open).
        let timeout_s = self.state.action_popup_timeout();
        if timeout_s > 0 {
            let pop = popover.clone();
            glib::timeout_add_seconds_local(timeout_s, move || {
                pop.popdown();
                glib::ControlFlow::Break
            });
        }
    }

    fn open_edit(self: &Rc<Self>, id: &str) {
        let Some(text) = self.state.entry_content(id) else {
            return;
        };
        let win = adw::Window::builder()
            .title("Editar conteúdo")
            .modal(true)
            .transient_for(&self.window)
            .default_width(440)
            .default_height(320)
            .build();

        let textview = gtk::TextView::new();
        textview.set_monospace(true);
        textview.set_wrap_mode(gtk::WrapMode::WordChar);
        textview.set_margin_top(8);
        textview.set_margin_bottom(8);
        textview.set_margin_start(8);
        textview.set_margin_end(8);
        textview.buffer().set_text(&text);

        let scroller = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&textview)
            .build();

        let cancel = gtk::Button::with_label("Cancelar");
        let save = gtk::Button::with_label("Salvar");
        save.add_css_class("suggested-action");
        let header = adw::HeaderBar::new();
        header.pack_start(&cancel);
        header.pack_end(&save);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.append(&header);
        vbox.append(&scroller);
        win.set_content(Some(&vbox));

        {
            let win = win.clone();
            cancel.connect_clicked(move |_| win.close());
        }
        {
            let (ui, id, win, textview) =
                (self.clone(), id.to_string(), win.clone(), textview.clone());
            save.connect_clicked(move |_| {
                let buffer = textview.buffer();
                let txt = buffer
                    .text(&buffer.start_iter(), &buffer.end_iter(), false)
                    .to_string();
                ui.state.replace_entry_text(&id, txt.trim());
                ui.rebuild(ui.search.text().as_str());
                win.close();
            });
        }
        self.dialog_open.set(true);
        {
            let ui = self.clone();
            win.connect_close_request(move |_| {
                ui.dialog_open.set(false);
                glib::Propagation::Proceed
            });
        }
        win.present();
    }

    fn open_qr(self: &Rc<Self>, id: &str) {
        let Some(text) = self.state.entry_content(id) else {
            return;
        };
        let Some(texture) = render_qr_texture(&text) else {
            return;
        };
        let win = adw::Window::builder()
            .title("QR Code")
            .modal(true)
            .transient_for(&self.window)
            .default_width(340)
            .default_height(380)
            .build();

        let picture = gtk::Picture::for_paintable(&texture);
        picture.set_size_request(280, 280);
        picture.set_margin_top(16);
        picture.set_margin_bottom(16);
        picture.set_margin_start(16);
        picture.set_margin_end(16);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
        vbox.append(&adw::HeaderBar::new());
        vbox.append(&picture);
        win.set_content(Some(&vbox));
        self.dialog_open.set(true);
        {
            let ui = self.clone();
            win.connect_close_request(move |_| {
                ui.dialog_open.set(false);
                glib::Propagation::Proceed
            });
        }
        win.present();
    }

    fn open_settings(self: &Rc<Self>) {
        let cfg = self.state.config_snapshot();
        let win = adw::PreferencesWindow::builder()
            .title("Configurações")
            .modal(true)
            .transient_for(&self.window)
            .default_width(440)
            .default_height(560)
            .build();
        let page = adw::PreferencesPage::new();

        // --- General ---
        let general = adw::PreferencesGroup::builder().title("Geral").build();

        let max_row = adw::SpinRow::with_range(1.0, 200.0, 1.0);
        max_row.set_title("Itens no histórico");
        max_row.set_value(cfg.general.max_items as f64);
        {
            let state = self.state.clone();
            max_row.connect_notify_local(Some("value"), move |row, _| {
                let v = row.value() as u32;
                state.update_config(|c| c.general.max_items = v);
            });
        }
        general.add(&max_row);

        general.add(
            &self.switch_row("Ignorar imagens", cfg.general.ignore_images, |c, v| {
                c.general.ignore_images = v
            }),
        );
        general.add(&self.switch_row(
            "Manter histórico entre reinícios",
            cfg.general.keep_clipboard_contents,
            |c, v| c.general.keep_clipboard_contents = v,
        ));
        general.add(&self.switch_row(
            "Ignorar seleção do mouse",
            cfg.general.ignore_selection,
            |c, v| c.general.ignore_selection = v,
        ));
        general.add(&self.switch_row(
            "Seleção do mouse apenas em texto",
            cfg.general.selection_text_only,
            |c, v| c.general.selection_text_only = v,
        ));
        general.add(&self.switch_row(
            "Sincronizar seleção e área de transferência",
            cfg.general.sync_clipboards,
            |c, v| c.general.sync_clipboards = v,
        ));
        general.add(&self.switch_row(
            "Evitar área de transferência vazia",
            cfg.general.prevent_empty_clipboard,
            |c, v| c.general.prevent_empty_clipboard = v,
        ));
        general.add(&self.switch_row(
            "Ações habilitadas",
            cfg.general.actions_enabled,
            |c, v| c.general.actions_enabled = v,
        ));
        general.add(&self.switch_row(
            "Ações mágicas por tipo de conteúdo",
            cfg.general.enable_magic_mime_actions,
            |c, v| c.general.enable_magic_mime_actions = v,
        ));
        general.add(&self.switch_row(
            "Repetir ação ao reusar item",
            cfg.general.replay_action_in_history,
            |c, v| c.general.replay_action_in_history = v,
        ));
        general.add(&self.switch_row(
            "Abrir no cursor (GNOME/X11)",
            cfg.general.popup_at_cursor,
            |c, v| c.general.popup_at_cursor = v,
        ));

        let timeout_row = adw::SpinRow::with_range(0.0, 60.0, 1.0);
        timeout_row.set_title("Tempo do menu de ações (s)");
        timeout_row.set_subtitle("0 = sem limite");
        timeout_row.set_value(cfg.general.timeout_for_action_popups as f64);
        {
            let state = self.state.clone();
            timeout_row.connect_notify_local(Some("value"), move |row, _| {
                let v = row.value() as u32;
                state.update_config(|c| c.general.timeout_for_action_popups = v);
            });
        }
        general.add(&timeout_row);
        page.add(&general);

        // --- Appearance ---
        let appearance = adw::PreferencesGroup::builder().title("Aparência").build();
        let scheme_row = adw::ComboRow::new();
        scheme_row.set_title("Tema");
        let model = gtk::StringList::new(&["Sistema", "Claro", "Escuro"]);
        scheme_row.set_model(Some(&model));
        scheme_row.set_selected(match cfg.ui.color_scheme {
            ColorScheme::System => 0,
            ColorScheme::Light => 1,
            ColorScheme::Dark => 2,
        });
        {
            let state = self.state.clone();
            scheme_row.connect_selected_notify(move |row| {
                let scheme = match row.selected() {
                    1 => ColorScheme::Light,
                    2 => ColorScheme::Dark,
                    _ => ColorScheme::System,
                };
                state.update_config(|c| c.ui.color_scheme = scheme);
                apply_color_scheme(scheme);
            });
        }
        appearance.add(&scheme_row);

        let font_row = adw::EntryRow::new();
        font_row.set_title("Fonte");
        font_row.set_text(&cfg.ui.font_family);
        font_row.set_show_apply_button(true);
        {
            let state = self.state.clone();
            font_row.connect_apply(move |row| {
                let family = row.text().to_string();
                state.update_config(|c| c.ui.font_family = family.clone());
                apply_font(&family);
            });
        }
        appearance.add(&font_row);
        page.add(&appearance);

        win.add(&page);
        self.dialog_open.set(true);
        {
            let ui = self.clone();
            win.connect_close_request(move |_| {
                ui.dialog_open.set(false);
                glib::Propagation::Proceed
            });
        }
        win.present();
    }

    fn switch_row(
        self: &Rc<Self>,
        title: &str,
        active: bool,
        apply: impl Fn(&mut klippo_core::Config, bool) + 'static,
    ) -> adw::SwitchRow {
        let row = adw::SwitchRow::new();
        row.set_title(title);
        row.set_active(active);
        let state = self.state.clone();
        row.connect_active_notify(move |row| {
            let v = row.is_active();
            state.update_config(|c| apply(c, v));
        });
        row
    }
}

/// A flat, hover-revealed icon button used in list rows.
fn row_button(icon: &str, tooltip: &str) -> gtk::Button {
    let btn = gtk::Button::from_icon_name(icon);
    btn.add_css_class("flat");
    btn.add_css_class("klippo-rowbtn");
    btn.set_valign(gtk::Align::Center);
    btn.set_tooltip_text(Some(tooltip));
    btn
}

/// Render `text` as a QR code into an RGBA [`gdk::Texture`].
fn render_qr_texture(text: &str) -> Option<gdk::Texture> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let scale = 8usize;
    let quiet = 4usize;
    let dim = (modules + 2 * quiet) * scale;

    let mut buf = vec![255u8; dim * dim * 4]; // white RGBA
    for y in 0..modules {
        for x in 0..modules {
            if colors[y * modules + x] == qrcode::Color::Dark {
                for dy in 0..scale {
                    for dx in 0..scale {
                        let py = (y + quiet) * scale + dy;
                        let px = (x + quiet) * scale + dx;
                        let idx = (py * dim + px) * 4;
                        buf[idx] = 0;
                        buf[idx + 1] = 0;
                        buf[idx + 2] = 0;
                        buf[idx + 3] = 255;
                    }
                }
            }
        }
    }

    let bytes = glib::Bytes::from(&buf);
    let texture = gdk::MemoryTexture::new(
        dim as i32,
        dim as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        dim * 4,
    );
    Some(texture.upcast())
}
