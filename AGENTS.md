# Repository Guidelines

## Project Structure & Module Organization

Klippo is a Cargo workspace. Crates live under `crates/`:

- `crates/klippo-core`: data model, config, SQLite store, search, and regex actions.
- `crates/klippo-capture`: clipboard capture/write backends for X11, Wayland, and GNOME bridge.
- `crates/klippo-dbus`: shared zbus D-Bus interfaces.
- `crates/klippo`: CLI, daemon, GTK4/libadwaita UI, and setup commands.

Desktop assets and packaging inputs are in `data/`: systemd, D-Bus, CSS, desktop entry, and bundled fonts. The GNOME Shell bridge is in `extension/`. User docs are `README.md` and `README.pt-BR.md`.

## Build, Test, and Development Commands

- `cargo build --workspace`: build all crates.
- `cargo build --release`: build optimized `target/release/klippo`.
- `cargo test --workspace`: run all Rust unit tests.
- `cargo clippy --workspace`: run lints for the workspace.
- `cargo fmt --all`: format all Rust code with rustfmt.
- `cargo run -p klippo -- daemon`: run the daemon locally.
- `cargo run -p klippo -- toggle`: show or hide the popup via the daemon.
- `cargo deb -p klippo`: build the Debian package.

Toolchain in `rust-toolchain.toml` uses Rust `1.95.0` with `rustfmt` and `clippy`.

## Coding Style & Naming Conventions

Use Rust 2021 style and let `cargo fmt --all` decide spacing and line wrapping. Name modules and files with `snake_case`, types and traits with `PascalCase`, and constants with `SCREAMING_SNAKE_CASE`. Keep crate boundaries clear: core logic in `klippo-core`, shared D-Bus types in `klippo-dbus`, capture backends in `klippo-capture`, and UI/CLI orchestration in `klippo`.

Prefer explicit error types in core code and `anyhow` at application boundaries. Avoid shell execution for actions unless the configured behavior explicitly requires it.

## Testing Guidelines

Tests are inline `#[cfg(test)]` modules beside the code they verify. Add unit tests for config parsing, store behavior, search, actions, and backend environment detection. Use descriptive names such as `deduplicates_existing_items` or `parses_default_config`. Run `cargo test --workspace` before submitting changes.

## Commit & Pull Request Guidelines

Recent history uses concise subjects, with Conventional Commit-style prefixes when helpful, for example `docs: rewrite README in US English`. Keep commits focused.

Pull requests should describe the behavior change, list commands run, and call out desktop/session coverage such as GNOME Wayland, X11, or wlroots. Include screenshots or short recordings for UI changes and link related issues when available.

## Security & Configuration Tips

Klippo stores config at `~/.config/klippo/config.toml` and history at `~/.local/share/klippo/history.db`. Do not commit local databases, generated packages, secrets, or user-specific config. Treat clipboard contents as sensitive in logs, tests, screenshots, and issues.
