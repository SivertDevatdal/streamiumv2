//! Handing a stream to a video player.
//!
//! The desktop shell does not render video itself yet — Media Foundation and
//! VA-API pipelines are Phase 4 work. Until then it drives a player the user
//! already has, passing the per-channel HTTP hints that IPTV playlists carry,
//! which is exactly what those players cannot work out on their own.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use streamium_core::model::HttpHints;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerKind {
    Mpv,
    Vlc,
    Ffplay,
    /// Something the user picked that we have no argument mapping for; it gets
    /// the URL and nothing else.
    Other,
}

impl PlayerKind {
    pub fn label(self) -> &'static str {
        match self {
            PlayerKind::Mpv => "mpv",
            PlayerKind::Vlc => "VLC",
            PlayerKind::Ffplay => "ffplay",
            PlayerKind::Other => "player",
        }
    }

    /// Whether arbitrary HTTP headers survive the command line.
    pub fn supports_headers(self) -> bool {
        matches!(self, PlayerKind::Mpv | PlayerKind::Ffplay)
    }

    fn from_stem(stem: &str) -> PlayerKind {
        match stem.to_ascii_lowercase().as_str() {
            "mpv" | "mpvnet" | "mpv-net" => PlayerKind::Mpv,
            "vlc" | "vlc-static" => PlayerKind::Vlc,
            "ffplay" => PlayerKind::Ffplay,
            _ => PlayerKind::Other,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    pub path: PathBuf,
    pub kind: PlayerKind,
}

impl Player {
    pub fn at(path: impl Into<PathBuf>) -> Player {
        let path = path.into();
        let kind = path
            .file_stem()
            .and_then(OsStr::to_str)
            .map(PlayerKind::from_stem)
            .unwrap_or(PlayerKind::Other);
        Player { path, kind }
    }

    pub fn display_name(&self) -> String {
        match self.kind {
            PlayerKind::Other => self.path.display().to_string(),
            kind => kind.label().to_string(),
        }
    }
}

/// Where players install themselves when nobody puts them on `PATH`.
#[cfg(windows)]
const WELL_KNOWN: &[&str] = &[
    r"C:\Program Files\VideoLAN\VLC\vlc.exe",
    r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe",
    r"C:\Program Files\mpv\mpv.exe",
    r"C:\Program Files\mpv.net\mpvnet.exe",
    r"C:\ProgramData\chocolatey\bin\mpv.exe",
    r"C:\ProgramData\chocolatey\bin\vlc.exe",
];

#[cfg(not(windows))]
const WELL_KNOWN: &[&str] = &[
    "/usr/bin/mpv",
    "/usr/local/bin/mpv",
    "/snap/bin/mpv",
    "/usr/bin/vlc",
    "/usr/local/bin/vlc",
    "/snap/bin/vlc",
    "/usr/bin/ffplay",
];

/// Names to look for on `PATH`, best first.
const CANDIDATES: &[&str] = &["mpv", "mpvnet", "vlc", "ffplay"];

/// Find the players installed on this machine, best first. mpv leads because
/// it handles raw transport streams and custom headers without complaint.
pub fn detect() -> Vec<Player> {
    let mut found: Vec<Player> = Vec::new();
    let mut add = |path: PathBuf| {
        if path.is_file() && !found.iter().any(|p| p.path == path) {
            found.push(Player::at(path));
        }
    };

    for name in CANDIDATES {
        for dir in path_dirs() {
            for exe in executable_names(name) {
                add(dir.join(exe));
            }
        }
    }
    for path in WELL_KNOWN {
        add(PathBuf::from(path));
    }
    // Scoop and per-user installs live under the profile directory.
    if let Some(home) = home_dir() {
        for rel in [
            "scoop/shims/mpv.exe",
            "scoop/shims/vlc.exe",
            "AppData/Local/Programs/mpv/mpv.exe",
            ".local/bin/mpv",
        ] {
            add(home.join(rel));
        }
    }

    found.sort_by_key(|p| match p.kind {
        PlayerKind::Mpv => 0,
        PlayerKind::Vlc => 1,
        PlayerKind::Ffplay => 2,
        PlayerKind::Other => 3,
    });
    found
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default()
}

fn executable_names(name: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{name}.exe"), format!("{name}.com")]
    } else {
        vec![name.to_string()]
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// Build the argument list for `player`, in the order it will be passed.
pub fn arguments(
    kind: PlayerKind,
    url: &str,
    title: &str,
    hints: &HttpHints,
    fallback_user_agent: &str,
) -> Vec<String> {
    let user_agent = hints
        .user_agent
        .clone()
        .unwrap_or_else(|| fallback_user_agent.to_string());
    let mut args = Vec::new();
    match kind {
        PlayerKind::Mpv => {
            args.push(format!("--title=Streamium — {title}"));
            args.push("--force-window=immediate".to_string());
            args.push(format!("--user-agent={user_agent}"));
            if let Some(referrer) = &hints.referrer {
                args.push(format!("--referrer={referrer}"));
            }
            if !hints.headers.is_empty() {
                let fields = hints
                    .headers
                    .iter()
                    .map(|(k, v)| format!("{k}: {v}"))
                    .collect::<Vec<_>>()
                    .join(",");
                args.push(format!("--http-header-fields={fields}"));
            }
            args.push("--".to_string());
            args.push(url.to_string());
        }
        PlayerKind::Vlc => {
            args.push(format!("--http-user-agent={user_agent}"));
            if let Some(referrer) = &hints.referrer {
                args.push(format!("--http-referrer={referrer}"));
            }
            args.push(url.to_string());
        }
        PlayerKind::Ffplay => {
            args.push("-user_agent".to_string());
            args.push(user_agent);
            let mut headers = String::new();
            if let Some(referrer) = &hints.referrer {
                headers.push_str(&format!("Referer: {referrer}\r\n"));
            }
            for (k, v) in &hints.headers {
                headers.push_str(&format!("{k}: {v}\r\n"));
            }
            if !headers.is_empty() {
                args.push("-headers".to_string());
                args.push(headers);
            }
            args.push("-window_title".to_string());
            args.push(format!("Streamium — {title}"));
            args.push(url.to_string());
        }
        PlayerKind::Other => args.push(url.to_string()),
    }
    args
}

/// Start `player` on `url`. The child is returned so the caller can tell
/// whether it is still running and reap it when it exits.
pub fn launch(
    player: &Player,
    url: &str,
    title: &str,
    hints: &HttpHints,
    fallback_user_agent: &str,
) -> std::io::Result<Child> {
    let args = arguments(player.kind, url, title, hints, fallback_user_agent);
    let mut command = Command::new(&player.path);
    command.args(&args);
    // ffplay is a console program: without this Windows puts a black console
    // window on screen alongside the video.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.spawn()
}

/// The command as the user would type it, for the diagnostics panel and for
/// reporting a launch that failed.
pub fn command_line(player: &Player, args: &[String]) -> String {
    let quote = |s: &str| {
        if s.contains(' ') || s.contains('"') {
            format!("\"{}\"", s.replace('"', "\\\""))
        } else {
            s.to_string()
        }
    };
    let mut line = quote(&player.path.display().to_string());
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    line
}

/// True when the path the user typed looks like it could be a player.
pub fn looks_executable(path: &str) -> bool {
    let p = Path::new(path.trim());
    !path.trim().is_empty() && p.is_file()
}
