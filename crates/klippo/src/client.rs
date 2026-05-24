//! Thin D-Bus client used by `klippo toggle|show|hide|clear`.
//!
//! Connects to the session bus and calls the running daemon's `Daemon1`
//! interface. Once a D-Bus activation file is installed, calling these will
//! auto-start the daemon if it isn't already running.

use anyhow::Result;
use klippo_dbus::{Capture1Proxy, Daemon1Proxy};

#[derive(Debug, Clone, Copy)]
pub enum Request {
    Toggle,
    Show,
    Hide,
    Clear,
}

pub fn run(request: Request) -> Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let conn = zbus::Connection::session().await?;
        let proxy = Daemon1Proxy::new(&conn).await?;
        match request {
            Request::Toggle => proxy.toggle().await?,
            Request::Show => proxy.show().await?,
            Request::Hide => proxy.hide().await?,
            Request::Clear => proxy.clear().await?,
        }
        Ok::<(), anyhow::Error>(())
    })
}

/// Read stdin and push it to the daemon as a text capture. Invoked by
/// `wl-paste --watch klippo __feed` on KDE/wlroots Wayland sessions.
pub fn feed() -> Result<()> {
    use std::io::Read;
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text)?;
    if text.trim().is_empty() {
        return Ok(());
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let conn = zbus::Connection::session().await?;
        let proxy = Capture1Proxy::new(&conn).await?;
        proxy.add_text(&text, "clipboard").await?;
        Ok::<(), anyhow::Error>(())
    })
}

/// Read stdin and push it to the daemon as a PNG image capture. Invoked by
/// `wl-paste --watch --type image/png klippo __feed-image`.
pub fn feed_image() -> Result<()> {
    use std::io::Read;
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes)?;
    if bytes.is_empty() {
        return Ok(());
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let conn = zbus::Connection::session().await?;
        let proxy = Capture1Proxy::new(&conn).await?;
        proxy.add_image("image/png", &bytes, "clipboard").await?;
        Ok::<(), anyhow::Error>(())
    })
}
