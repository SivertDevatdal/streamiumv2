//! Persisted settings: the user's sources, their player, their favourites.
//!
//! Everything lives in one JSON file next to the platform's other per-user
//! application data (`%APPDATA%\Streamium` on Windows, `~/.config/streamium`
//! elsewhere). Saving is best-effort: a settings file that cannot be written
//! must never stop the player from running.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use streamium_core::model::Source;

/// Per-user directory holding `config.json`.
pub fn config_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("Streamium");
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("streamium");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("streamium");
    }
    PathBuf::from(".")
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

/// One source the user added, plus the state we keep for it between runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceEntry {
    pub source: Source,
    /// Guide URL the user typed, overriding anything the source advertises.
    #[serde(default)]
    pub epg_url: Option<String>,
    /// Pull the movie catalogue as well as live TV. Off by default: VOD
    /// listings run to six figures on big providers and slow the first load.
    #[serde(default)]
    pub include_vod: bool,
    /// Channel ids the user starred, by [`streamium_core::model::Channel::id`].
    #[serde(default)]
    pub favourites: Vec<String>,
}

impl SourceEntry {
    pub fn new(source: Source) -> Self {
        SourceEntry {
            source,
            epg_url: None,
            include_vod: false,
            favourites: Vec::new(),
        }
    }

    pub fn name(&self) -> &str {
        self.source.name()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub sources: Vec<SourceEntry>,
    /// Index into `sources` of the one to open on launch.
    pub last_source: Option<usize>,
    /// Full path to the external player executable. Empty means "detect".
    pub player_path: String,
    /// Ask Xtream servers for HLS instead of raw transport streams.
    pub prefer_hls: bool,
    /// Reload the selected source when the application starts.
    pub refresh_on_launch: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            sources: Vec::new(),
            last_source: None,
            player_path: String::new(),
            prefer_hls: false,
            refresh_on_launch: true,
        }
    }
}

impl Config {
    pub fn load() -> Config {
        let path = config_path();
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Config::default();
        };
        serde_json::from_str(&text).unwrap_or_else(|e| {
            // A settings file from a newer build, or one a user hand-edited
            // into invalid JSON, must not be silently destroyed: keep it as
            // `.bak` so it can be recovered, and start fresh.
            eprintln!("settings file could not be read ({e}); starting with defaults");
            let _ = std::fs::rename(&path, path.with_extension("json.bak"));
            Config::default()
        })
    }

    pub fn save(&self) -> std::io::Result<()> {
        let dir = config_dir();
        std::fs::create_dir_all(&dir)?;
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        // Write and rename so an interrupted save cannot truncate the file.
        let tmp = dir.join("config.json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, config_path())
    }

    pub fn selected(&self) -> Option<&SourceEntry> {
        self.last_source.and_then(|i| self.sources.get(i))
    }
}
