//! klippo-core: GUI/D-Bus-free core of Klippo.
//!
//! Houses the clipboard entry model, the persistent history store (SQLite),
//! deduplication and MRU ordering, incremental search, the TOML configuration
//! (mirroring Klipper's `klipperrc` defaults), and the regex Actions subsystem.

pub mod actions;
pub mod config;
pub mod error;
pub mod model;
pub mod paths;
pub mod search;
pub mod store;

pub use config::Config;
pub use error::{Error, Result};
pub use model::{Entry, EntryKind, Source};
pub use store::Store;
