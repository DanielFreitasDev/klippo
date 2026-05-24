# Klippo

**A KDE Klipper–style clipboard history manager for Linux.** Built in Rust with
GTK4/libadwaita, working on both **Wayland** and **X11**, with first-class
support for **Ubuntu/GNOME**.

**English** · [Português 🇧🇷](README.pt-BR.md)

Klippo keeps a searchable history of everything you copy — text and images — and
pops it up at your cursor with **Super+V**, just like KDE's Klipper, but designed
to feel at home on GNOME and other Linux desktops.

> **Why a GNOME Shell extension?** On GNOME Wayland the compositor (Mutter)
> implements neither `wlr-data-control` nor `ext-data-control`, so **no external
> app can monitor the clipboard**. Only code running inside GNOME Shell can —
> which is exactly how GPaste, Pano and Clipboard Indicator work. Klippo ships a
> tiny extension that captures clipboard changes and forwards them to the Rust
> daemon over D-Bus. On X11, KDE Plasma and wlroots compositors the daemon
> captures directly and the extension isn't needed.

## Features

- 📋 **Text & image history** — most-recent-first, automatic de-duplication,
  configurable size (default **25**), copies text or images back to the
  clipboard, and persists across reboots.
- 🔍 **Real-time search** — start typing to filter; clear the box to restore the
  full list.
- ⌨️ **Numbered items** (1–9) with **Alt+1…Alt+9** quick-select; arrow keys and
  Enter work too.
- 🖱️ **Opens at the cursor** and **closes when it loses focus** (click outside),
  Klipper-style. *(Cursor placement on GNOME Wayland is performed by the
  extension — see the support table.)*
- 📌 **Pin** important entries — pinned items stay above the most-recent order.
- 🧰 **Per-item buttons** revealed on hover: **pin**, run a matching action, show
  a **QR code**, **edit** the entry, or **delete** it — plus **Clear all**.
- 🎨 **Light/dark** following the system theme, with a configurable font
  (**JetBrains Mono** bundled).
- ⚙️ **Settings dialog** — history size, ignore images, ignore mouse selection,
  sync selection↔clipboard, prevent empty clipboard, actions on/off, magic
  actions, replay-on-reuse, action-menu timeout, open-at-cursor, font and theme.
- 🤖 **Regex Actions** (like Klipper) — match clipboard text and run commands
  with `%s` / `%0`–`%9` placeholders. **Shell-free by default** (injection-safe),
  with an optional auto-popup menu. Built-in **magic actions** detect URLs,
  e-mails and file paths automatically.
- ♻️ **Live config reload** — edits to `config.toml` apply without a restart.

## Supported environments

| Desktop / session | Clipboard capture | Super+V | Open at cursor |
|---|---|---|---|
| **GNOME (Wayland)** — Ubuntu, Fedora, … | ✅ GNOME Shell extension (text + PNG images) | ✅ set up automatically | ✅ |
| **X11** (any desktop) | ✅ polling CLIPBOARD + PRIMARY (text + images) | ⚙️ bind `klippo toggle` | ➖ placed by the WM |
| **KDE Plasma 6 / wlroots** (Sway, Hyprland) | ✅ `wl-paste --watch` for text + PNG images (needs `wl-clipboard`) | ⚙️ bind `klippo toggle` | ➖ placed by the compositor |

> The primary development and testing target is **GNOME on Wayland (Ubuntu)**.
> The X11 and Wayland data-control backends are implemented; testing on those
> sessions is very welcome.

## Install

### 1. Install Rust and the build dependencies

Klippo needs **Rust 1.95+** (via [rustup](https://rustup.rs)) and the GTK4 /
libadwaita development libraries. `wl-clipboard` is only needed on KDE/wlroots
Wayland sessions.

**Ubuntu / Debian**
```bash
sudo apt install build-essential libgtk-4-dev libadwaita-1-dev wl-clipboard
```

**Fedora**
```bash
sudo dnf install gtk4-devel libadwaita-devel wl-clipboard
```

**Arch / Manjaro**
```bash
sudo pacman -S --needed base-devel gtk4 libadwaita wl-clipboard
```

If you don't have Rust yet:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. Clone and build

```bash
git clone https://github.com/DanielFreitasDev/klippo.git
cd klippo
cargo build --release        # → target/release/klippo
```

### 3a. Install via `.deb` (Debian/Ubuntu — recommended there)

```bash
cargo install cargo-deb       # once
cargo deb -p klippo           # builds target/debian/klippo_*_amd64.deb
sudo dpkg -i target/debian/klippo_*_amd64.deb
```

The package installs the binary, a systemd **user** service, a D-Bus activation
file, an autostart entry, and the bundled JetBrains Mono font.

### 3b. Install manually (Fedora, Arch, others)

Replicate what the `.deb` does — copy the binary and the integration files:

```bash
sudo install -Dm755 target/release/klippo /usr/bin/klippo
# D-Bus activation (auto-starts the daemon on the first `klippo toggle`):
sudo install -Dm644 data/org.klippo.Daemon.service \
  /usr/share/dbus-1/services/org.klippo.Daemon.service
# autostart the daemon at login:
sudo install -Dm644 data/klippo.desktop /etc/xdg/autostart/klippo.desktop
```

## First-time setup (GNOME)

```bash
klippo setup
```

This installs the JetBrains Mono font, installs and enables the GNOME Shell
extension, and binds **Super+V** to `klippo toggle` (freeing `<Super>v` from
GNOME's message-tray toggle, which stays on `<Super>m`). Then **log out and back
in** so GNOME Shell loads the extension. Copy something and press **Super+V**.

Granular commands: `klippo install-extension`, `klippo keybinding`.

On **X11** or **wlroots/KDE**, the extension isn't needed — just bind a shortcut
to `klippo toggle` in your desktop/WM settings and make sure `klippo daemon` runs
at login (the `.deb` autostarts it).

## Usage

| Action | How |
|---|---|
| Open / close the popup | **Super+V** (or `klippo toggle`) |
| Filter | start typing |
| Pick an item | click, **Enter** (top match), or **Alt+1…9** |
| Pin / action / QR / edit / delete (per item) | hover the row |
| Clear all · Settings | footer buttons |
| Dismiss | **Esc** or click outside |

Selecting an item copies its content (text or image) to the clipboard (it does
**not** auto-paste) and moves it to the top — same as Klipper.

## Configuration

Config lives at `~/.config/klippo/config.toml` (created with Klipper-like
defaults on first run); history is stored at `~/.local/share/klippo/history.db`.
The daemon watches the file and **reloads changes live**.

```toml
[general]
max_items = 25
keep_clipboard_contents = true
ignore_images = true
ignore_selection = true            # don't capture mouse (PRIMARY) selections
selection_text_only = true         # PRIMARY selections only store text
sync_clipboards = false
prevent_empty_clipboard = true
actions_enabled = true
enable_magic_mime_actions = true   # suggest actions for URLs / e-mails / files
replay_action_in_history = false   # re-run the action when re-selecting an item
timeout_for_action_popups = 8      # auto-close the action menu after N s (0 = never)
popup_at_cursor = false            # open at the pointer (GNOME via extension / X11)

[ui]
color_scheme = "system"          # system | light | dark
font_family = "JetBrains Mono"
popup_width = 380
popup_max_rows = 12

# Regex action example — runs shell-free (injection-safe):
[[actions]]
name = "Open URL"
regex = '^(https?://\S+)$'
automatic = false                # true = pop the action menu automatically
  [[actions.commands]]
  command = "xdg-open %s"        # %s = whole match, %1..%9 = capture groups
  output = "ignore"              # ignore | replace-clipboard | new-entry
```

## Architecture

Klippo is a Cargo workspace of four crates plus a GNOME Shell extension:

```
GNOME Shell extension (GJS) ──D-Bus (Capture1)──┐
  captures clipboard on Wayland                 ▼
X11 / KDE / wlroots ── direct capture ───►  klippo-core (daemon)
  (arboard / wl-paste)                      • SQLite history, dedup + MRU
                                            • TOML config, regex Actions
                                            • zbus service (org.klippo.Daemon)
                                                    │ async-channel (UiEvent)
                                                    ▼
                                            GTK4 + libadwaita popup
                                    Super+V → gsettings → `klippo toggle`
```

- **`klippo-core`** — model, SQLite store (dedup + MRU + pruning), search, TOML
  config, regex Actions. No GUI/D-Bus; unit-tested.
- **`klippo-dbus`** — shared zbus interfaces: `Daemon1` (control/query) and
  `Capture1` (capture push).
- **`klippo-capture`** — the `ClipboardSource` trait, environment detection, and
  the X11 / Wayland-data-control / GNOME-bridge backends.
- **`klippo`** — the binary: daemon, GTK4 popup, CLI, and the GNOME `setup`.
- **`extension/`** — the GNOME Shell bridge (capture + cursor placement).

## Development

```bash
cargo test --workspace      # unit tests
cargo clippy --workspace    # lints
cargo fmt --all             # format
```

## Uninstall

```bash
sudo apt remove klippo                       # the package
# GNOME extension + shortcut (if you ran `klippo setup`):
gnome-extensions disable klippo@klippo.org
rm -rf ~/.local/share/gnome-shell/extensions/klippo@klippo.org
gsettings reset org.gnome.shell.keybindings toggle-message-tray
# then remove the "klippo" entry under Settings → Keyboard → Custom Shortcuts
```

## License

Licensed under [GPL-3.0-or-later](https://www.gnu.org/licenses/gpl-3.0.html).
Bundles the **JetBrains Mono** font under the SIL Open Font License 1.1
(`data/fonts/OFL.txt`).

## Acknowledgements

Inspired by KDE's **Klipper**. Monospace font by **JetBrains**.
