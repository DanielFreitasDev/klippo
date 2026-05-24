//! First-run setup: install the GNOME Shell extension and bind Super+V.
//!
//! Both steps are GNOME-specific. The extension files are embedded in the
//! binary and written to the user's extensions directory. The keybinding step
//! frees `<Super>v` from GNOME's message-tray toggle and points a custom
//! shortcut at `klippo toggle`.

use std::process::Command;

use anyhow::{Context, Result};
use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;

const EXT_UUID: &str = "klippo@klippo.org";
const EXT_METADATA: &str = include_str!("../../../extension/metadata.json");
const EXT_JS: &str = include_str!("../../../extension/extension.js");

// Bundled JetBrains Mono (OFL-1.1), installed to the user font dir on setup.
const FONT_FILES: &[(&str, &[u8])] = &[
    (
        "JetBrainsMono-Regular.ttf",
        include_bytes!("../../../data/fonts/JetBrainsMono-Regular.ttf"),
    ),
    (
        "JetBrainsMono-Bold.ttf",
        include_bytes!("../../../data/fonts/JetBrainsMono-Bold.ttf"),
    ),
    (
        "JetBrainsMono-Italic.ttf",
        include_bytes!("../../../data/fonts/JetBrainsMono-Italic.ttf"),
    ),
    (
        "JetBrainsMono-BoldItalic.ttf",
        include_bytes!("../../../data/fonts/JetBrainsMono-BoldItalic.ttf"),
    ),
];

const TRAY_SCHEMA: &str = "org.gnome.shell.keybindings";
const TRAY_KEY: &str = "toggle-message-tray";
const MEDIA_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys";
const CUSTOM_LIST_KEY: &str = "custom-keybindings";
const CUSTOM_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const KLIPPO_KB_PATH: &str =
    "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/klippo/";

/// Run the full setup: install the font + extension and bind Super+V.
pub fn run() -> Result<()> {
    install_font()?;
    println!();
    install_extension()?;
    println!();
    keybinding()?;
    println!();
    println!("✓ Pronto. Próximos passos:");
    println!("  1. Faça logout e login (o GNOME Shell no Wayland só carrega a");
    println!("     extensão após reiniciar a sessão).");
    println!("  2. Inicie o daemon:  klippo daemon");
    println!("  3. Copie algo e pressione Super+V para abrir o histórico.");
    Ok(())
}

/// Install the bundled JetBrains Mono into the user font dir and refresh caches.
pub fn install_font() -> Result<()> {
    let dir = glib::user_data_dir().join("fonts").join("klippo");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    for (name, bytes) in FONT_FILES {
        std::fs::write(dir.join(name), bytes)?;
    }
    // Best-effort cache refresh; fontconfig also picks up ~/.local/share/fonts.
    let _ = Command::new("fc-cache").arg("-f").arg(&dir).status();
    println!("✓ Fonte JetBrains Mono instalada em {}", dir.display());
    Ok(())
}

/// Write the embedded extension to the user's extensions dir and enable it.
pub fn install_extension() -> Result<()> {
    let dest = glib::user_data_dir()
        .join("gnome-shell")
        .join("extensions")
        .join(EXT_UUID);
    std::fs::create_dir_all(&dest).with_context(|| format!("creating {}", dest.display()))?;
    std::fs::write(dest.join("metadata.json"), EXT_METADATA)?;
    std::fs::write(dest.join("extension.js"), EXT_JS)?;
    println!("✓ Extensão instalada em {}", dest.display());

    // Enable it (ignore failure: it may need a relog before it can be enabled).
    match Command::new("gnome-extensions")
        .args(["enable", EXT_UUID])
        .status()
    {
        Ok(s) if s.success() => println!("✓ Extensão habilitada ({EXT_UUID})"),
        _ => println!(
            "• Não foi possível habilitar agora. Após o relog, rode:\n    gnome-extensions enable {EXT_UUID}"
        ),
    }
    Ok(())
}

/// Bind `<Super>v` to `klippo toggle`, freeing it from the message-tray toggle.
pub fn keybinding() -> Result<()> {
    if !schema_installed(TRAY_SCHEMA) || !schema_installed(MEDIA_SCHEMA) {
        anyhow::bail!(
            "esquemas gsettings do GNOME não encontrados — o atalho automático \
             só é suportado no GNOME. Em outros ambientes, configure o atalho \
             para 'klippo toggle' manualmente."
        );
    }

    // 1. Remove <Super>v from the message-tray toggle (keep any other binding).
    let tray = gio::Settings::new(TRAY_SCHEMA);
    let mut tray_bindings: Vec<String> =
        tray.strv(TRAY_KEY).iter().map(|s| s.to_string()).collect();
    if tray_bindings.iter().any(|b| b == "<Super>v") {
        tray_bindings.retain(|b| b != "<Super>v");
        let refs: Vec<&str> = tray_bindings.iter().map(String::as_str).collect();
        tray.set_strv(TRAY_KEY, refs)?;
        println!("✓ Liberado <Super>v (bandeja de notificações segue em <Super>m)");
    }

    // 2. Register our custom keybinding path.
    let media = gio::Settings::new(MEDIA_SCHEMA);
    let mut list: Vec<String> = media
        .strv(CUSTOM_LIST_KEY)
        .iter()
        .map(|s| s.to_string())
        .collect();
    if !list.iter().any(|p| p == KLIPPO_KB_PATH) {
        list.push(KLIPPO_KB_PATH.to_string());
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        media.set_strv(CUSTOM_LIST_KEY, refs)?;
    }

    let custom = gio::Settings::with_path(CUSTOM_SCHEMA, KLIPPO_KB_PATH);
    let exe = std::env::current_exe()?.to_string_lossy().into_owned();
    custom.set_string("name", "Klippo")?;
    custom.set_string("command", &format!("{exe} toggle"))?;
    custom.set_string("binding", "<Super>v")?;
    gio::Settings::sync();

    println!("✓ Super+V configurado para abrir o Klippo ('{exe} toggle')");
    Ok(())
}

fn schema_installed(id: &str) -> bool {
    gio::SettingsSchemaSource::default()
        .and_then(|src| src.lookup(id, true))
        .is_some()
}
