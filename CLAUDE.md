# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Klippo is a KDE Klipper–style clipboard history manager for Linux, built with GTK4/libadwaita. It targets Wayland and X11; the primary development target is **GNOME on Wayland (Ubuntu)**. It is a Cargo workspace of four crates plus a GNOME Shell extension.

## Commands

```bash
cargo build --release        # → target/release/klippo (needs libgtk-4-dev libadwaita-1-dev)
cargo test --workspace       # all unit tests (live in klippo-core and klippo-capture)
cargo test -p klippo-core dedup_promotes_existing_to_top   # a single test by name
cargo clippy --workspace
cargo fmt --all
cargo run -p klippo -- daemon         # run the daemon (capture + history + D-Bus + UI) for dev
cargo run -p klippo -- toggle         # drive a running daemon over D-Bus
cargo deb -p klippo          # build the .deb (needs `cargo install cargo-deb`)
```

The toolchain is pinned to **Rust 1.95.0** (`rust-toolchain.toml`). Tests are inline `#[cfg(test)]` modules — the `klippo` binary crate has none; logic worth testing belongs in `klippo-core`. Set `RUST_LOG=klippo=debug` to raise log verbosity (default filter is `klippo=info,warn`).

## Architecture

### One binary, several subcommands (`crates/klippo/src/main.rs`)

`klippo daemon` is the only long-lived process: it runs clipboard capture, the SQLite history, the D-Bus service, **and** the GTK4 UI all in one process. Every other subcommand is a thin D-Bus client of that daemon:
- `toggle`/`show`/`hide`/`clear` → call the `Daemon1` interface (`client.rs`). A D-Bus activation file auto-starts the daemon if needed.
- `setup`/`install-extension`/`keybinding` → GNOME-only first-run setup (`setup.rs`).
- `__feed` (hidden) → reads stdin and pushes it as a capture; invoked by `wl-paste --watch`.

### The daemon's threading model (`crates/klippo/src/daemon.rs`)

This is the central thing to understand before editing the daemon or UI:
- **GTK owns the main thread** (`ui.rs`). A background **multi-threaded Tokio runtime** hosts the zbus service and capture backends.
- **daemon → UI**: an `async-channel` of `UiEvent`s, awaited on the GTK main loop.
- **UI → state**: direct *synchronous* calls into `AppState` (SQLite is fast; `AppState` is `Send + Sync`).
- `AppState` is the single owner of the store (`Mutex<Store>`) and config (`RwLock<Config>`). Both D-Bus interfaces and the UI share it via `Arc`.

Two D-Bus interfaces are served at the same bus name/path (`org.klippo.Daemon` @ `/org/klippo/Daemon`): `Daemon1` (control + query, used by the CLI and UI) and `Capture1` (capture push, called by the GNOME extension). Their **proxy/client side** is defined once in `klippo-dbus`; the daemon implements the server side against `AppState`.

### Why the GNOME Shell extension exists (`extension/`)

On GNOME Wayland, Mutter implements neither `wlr-data-control` nor `ext-data-control`, so **no external app can monitor the clipboard**. Only code inside GNOME Shell can. The extension (GJS/ESM, no UI, stores nothing) watches the CLIPBOARD selection and forwards changes to the daemon over `Capture1`. It also positions the popup at the cursor (Wayland clients cannot self-position) by finding the window titled `"Klippo"`.

### Backend abstraction (`crates/klippo-capture/`)

`detect_backend()` picks a backend from `XDG_SESSION_TYPE` + `XDG_CURRENT_DESKTOP` (override with `KLIPPO_BACKEND=x11|wayland-dc|gnome|none`):
- **GnomeBridge** — `NullSource`/`NullWriter`: the daemon neither captures nor writes the clipboard. Capture arrives via the extension over D-Bus; writes are done by the focused GTK popup (`UiEvent::SetClipboard`).
- **WaylandDataControl** (KDE 6.4+/wlroots) — spawns `wl-paste --watch klippo __feed`.
- **X11** — `arboard` polling on a dedicated OS thread.

### `klippo-core` — GUI/D-Bus-free core

Holds the model, store, search, config, and actions. No GTK or zbus deps; this is where unit-tested logic lives.

## Cross-cutting invariants (easy to break)

- **Content hash is the primary key.** An `Entry`'s `id` is the BLAKE3 hash of its content (`model.rs`), so dedup is automatic: re-copying identical content is an upsert that bumps the timestamp (MRU promotion), never a new row. Image entries hash their bytes; PNGs + thumbnails live under `~/.local/share/klippo/{images,thumbs}` and are GC'd when an entry is removed or pruned.
- **Actions are injection-safe by default** (`actions.rs`). The command template is split into argv tokens *before* `%s`/`%0`–`%9` substitution, so clipboard content can never spill into adjacent argv slots or introduce new tokens. `shell = true` opts back into `/bin/sh -c` and loses this — keep it rare. Preserve this ordering (split-then-substitute) in any change here.
- **Select copies, does not paste** — and promotes the entry to the top. Matches Klipper.
- **Config tolerates partial/old files.** Every config struct is `#[serde(default)]`, so missing fields and new options across versions load cleanly. Defaults intentionally mirror Klipper's `klipperrc` (`config.rs`).
- Config: `~/.config/klippo/config.toml`; history DB (SQLite, WAL): `~/.local/share/klippo/history.db`. Paths centralized in `klippo-core/src/paths.rs`.

## Conventions

- **Comments and docs are in English; user-facing strings (GTK UI, CLI help text) are Brazilian Portuguese.** Match this when editing — don't "fix" Portuguese UI text to English.
- The GTK app is launched with `run_with_args::<&str>(&[])` so the clap subcommand never reaches `GApplication`. Don't pass real args through.
