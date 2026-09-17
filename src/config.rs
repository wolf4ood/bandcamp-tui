//! On-disk configuration. Never contains the session cookie (see `secrets`).

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::secrets::SecretStoreKind;

const APP_NAME: &str = "bandcamp-tui";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Bandcamp username of the last validated session (for greeting before the network round-trip).
    pub username: Option<String>,
    /// Where the session cookie is stored.
    pub secret_store: SecretStoreKind,
    /// Playback volume in percent.
    pub volume: u8,
    /// Publish playback on the D-Bus session bus (MPRIS). Linux only.
    pub mpris: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            username: None,
            secret_store: SecretStoreKind::Keyring,
            volume: 80,
            mpris: true,
        }
    }
}

fn project_dirs() -> Result<ProjectDirs> {
    ProjectDirs::from("", "", APP_NAME).context("could not determine the home directory")
}

pub fn config_path() -> Result<PathBuf> {
    Ok(project_dirs()?.config_dir().join("config.toml"))
}

pub fn data_dir() -> Result<PathBuf> {
    Ok(project_dirs()?.data_dir().to_path_buf())
}

/// The play queue, restored on the next start (see `player::store`).
pub fn queue_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("queue.json"))
}

/// Directory for the log file: `$XDG_STATE_HOME` on Linux, the local data dir elsewhere.
pub fn log_dir() -> Result<PathBuf> {
    let dirs = project_dirs()?;
    Ok(dirs
        .state_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dirs.data_local_dir().to_path_buf()))
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path()?;
        match fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = config_path()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serializing config")?;
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))
    }
}
