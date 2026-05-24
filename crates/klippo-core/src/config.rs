//! TOML configuration, mirroring Klipper's `klipperrc` options and defaults.

use serde::{Deserialize, Serialize};

use crate::actions::{Action, ActionCommand, OutputMode};
use crate::error::Result;
use crate::paths;

/// Top-level configuration, persisted to `~/.config/klippo/config.toml`.
///
/// `#[serde(default)]` on each struct means any missing field falls back to its
/// `Default`, so partial config files (or new options across versions) load
/// cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub general: General,
    pub ui: Ui,
    pub shortcuts: Shortcuts,
    pub actions: Vec<Action>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: General::default(),
            ui: Ui::default(),
            shortcuts: Shortcuts::default(),
            actions: default_actions(),
        }
    }
}

impl Config {
    /// Load config from disk, writing defaults first if the file is absent.
    pub fn load() -> Result<Config> {
        let path = paths::config_path();
        if !path.exists() {
            let cfg = Config::default();
            cfg.save()?;
            return Ok(cfg);
        }
        let text = std::fs::read_to_string(&path)?;
        Ok(toml::from_str(&text)?)
    }

    /// Persist config to disk (creating the directory if needed).
    pub fn save(&self) -> Result<()> {
        let path = paths::config_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, toml::to_string_pretty(self)?)?;
        Ok(())
    }
}

/// General behavior, mirroring `klipperrc` keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct General {
    /// `MaxClipItems` — how many entries to keep (Klipper default 20; we use 25).
    pub max_items: u32,
    /// `KeepClipboardContents` — persist history across restarts.
    pub keep_clipboard_contents: bool,
    /// `SyncClipboards` — keep PRIMARY and CLIPBOARD identical.
    pub sync_clipboards: bool,
    /// `IgnoreSelection` — don't record mouse selections (PRIMARY).
    pub ignore_selection: bool,
    /// `SelectionTextOnly` — only store text selections, never image selections.
    pub selection_text_only: bool,
    /// `IgnoreImages` — never store images (Klipper default true).
    pub ignore_images: bool,
    /// `PreventEmptyClipboard` — restore the top item if the clipboard is cleared.
    pub prevent_empty_clipboard: bool,
    /// `StripWhiteSpace` — trim before running actions.
    pub strip_whitespace: bool,
    /// Master on/off for the Actions feature (Ctrl+Alt+X in Klipper).
    pub actions_enabled: bool,
    /// `ReplayActionInHistory` — re-trigger actions when picking an old item.
    pub replay_action_in_history: bool,
    /// `EnableMagicMimeActions` — suggest actions based on file MIME type.
    pub enable_magic_mime_actions: bool,
    /// `TimeoutForActionPopups` — seconds the action menu stays open.
    pub timeout_for_action_popups: u32,
    /// `PopupMenuAtCursor` — open the popup at the cursor (X11 / GNOME ext only).
    pub popup_at_cursor: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            max_items: 25,
            keep_clipboard_contents: true,
            sync_clipboards: false,
            ignore_selection: true,
            selection_text_only: true,
            ignore_images: true,
            prevent_empty_clipboard: true,
            strip_whitespace: true,
            actions_enabled: true,
            replay_action_in_history: false,
            enable_magic_mime_actions: true,
            timeout_for_action_popups: 8,
            popup_at_cursor: false,
        }
    }
}

/// How the system should pick light vs dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ColorScheme {
    /// Follow the desktop preference (libadwaita `AdwStyleManager`).
    #[default]
    System,
    Light,
    Dark,
}

/// Appearance settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub font_family: String,
    pub color_scheme: ColorScheme,
    pub popup_width: u32,
    pub popup_max_rows: u32,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            font_family: "JetBrains Mono".to_string(),
            color_scheme: ColorScheme::System,
            popup_width: 380,
            popup_max_rows: 12,
        }
    }
}

/// Keyboard shortcut preferences. The toggle is documented here but actually
/// bound through the desktop (gsettings custom keybinding on GNOME).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shortcuts {
    pub toggle: String,
}

impl Default for Shortcuts {
    fn default() -> Self {
        Self {
            toggle: "<Super>v".to_string(),
        }
    }
}

/// A single safe example action shipped on first run.
fn default_actions() -> Vec<Action> {
    vec![Action {
        name: "Abrir URL".to_string(),
        regex: r"^(https?://\S+)$".to_string(),
        strip_whitespace: true,
        automatic: false,
        commands: vec![ActionCommand {
            command: "xdg-open %s".to_string(),
            output: OutputMode::Ignore,
            shell: false,
        }],
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_klipper() {
        let c = Config::default();
        assert_eq!(c.general.max_items, 25);
        assert!(c.general.ignore_images);
        assert!(c.general.prevent_empty_clipboard);
        assert_eq!(c.ui.font_family, "JetBrains Mono");
        assert_eq!(c.actions.len(), 1);
    }

    #[test]
    fn partial_toml_fills_defaults() {
        // Only override one value; everything else must fall back to defaults.
        let cfg: Config = toml::from_str("[general]\nmax_items = 50\n").unwrap();
        assert_eq!(cfg.general.max_items, 50);
        assert!(cfg.general.ignore_images); // default preserved
        assert_eq!(cfg.ui.popup_width, 380); // whole [ui] table defaulted
    }

    #[test]
    fn roundtrips_through_toml() {
        let original = Config::default();
        let text = toml::to_string_pretty(&original).unwrap();
        let parsed: Config = toml::from_str(&text).unwrap();
        assert_eq!(original, parsed);
    }
}
