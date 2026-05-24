//! Klippo entry point: a single binary with subcommands.
//!
//! `klippo daemon` runs the long-lived service (capture + history + D-Bus, and
//! the UI once added). The other subcommands are thin D-Bus clients that drive
//! the running daemon — these are what the Super+V keybinding invokes.

mod client;
mod daemon;
mod setup;
mod ui;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "klippo",
    version,
    about = "Klippo — clone do Klipper para Linux (Wayland + X11)"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Inicia o daemon: captura, histórico, D-Bus (e a UI quando disponível).
    Daemon,
    /// Mostra a janela se oculta, oculta se visível.
    Toggle,
    /// Mostra a janela do histórico.
    Show,
    /// Oculta a janela do histórico.
    Hide,
    /// Limpa todo o histórico.
    Clear,
    /// Configura tudo no GNOME: instala a extensão e liga o atalho Super+V.
    Setup,
    /// Instala/ativa apenas a extensão do GNOME Shell.
    InstallExtension,
    /// Configura apenas o atalho Super+V (gsettings).
    Keybinding,
    /// (interno) Lê stdin e o envia como captura de texto — usado por `wl-paste --watch`.
    #[command(name = "__feed", hide = true)]
    Feed,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "klippo=info,warn".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Daemon => daemon::run(),
        Command::Toggle => client::run(client::Request::Toggle),
        Command::Show => client::run(client::Request::Show),
        Command::Hide => client::run(client::Request::Hide),
        Command::Clear => client::run(client::Request::Clear),
        Command::Setup => setup::run(),
        Command::InstallExtension => setup::install_extension(),
        Command::Keybinding => setup::keybinding(),
        Command::Feed => client::feed(),
    }
}
