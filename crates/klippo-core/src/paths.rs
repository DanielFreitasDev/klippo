//! XDG paths for Klippo's config, database and image storage.

use std::path::PathBuf;

use directories::ProjectDirs;

fn project_dirs() -> ProjectDirs {
    // On Linux this resolves to ~/.config/klippo and ~/.local/share/klippo.
    ProjectDirs::from("", "", "klippo").expect("could not determine the user home directory")
}

/// `~/.local/share/klippo`
pub fn data_dir() -> PathBuf {
    project_dirs().data_dir().to_path_buf()
}

/// `~/.config/klippo`
pub fn config_dir() -> PathBuf {
    project_dirs().config_dir().to_path_buf()
}

/// `~/.config/klippo/config.toml`
pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

/// `~/.local/share/klippo/history.db`
pub fn db_path() -> PathBuf {
    data_dir().join("history.db")
}

/// `~/.local/share/klippo/images`
pub fn images_dir() -> PathBuf {
    data_dir().join("images")
}

/// `~/.local/share/klippo/thumbs`
pub fn thumbs_dir() -> PathBuf {
    data_dir().join("thumbs")
}
