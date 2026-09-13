//! Turning a source into a catalogue and a guide, on a worker thread.
//!
//! Every step reports progress back to the interface so a 20 000 channel
//! provider with a 200 MB guide shows what it is doing instead of freezing.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::sync::mpsc::Sender;
use std::time::Instant;

use streamium_core::catalog::Catalog;
use streamium_core::epg::EpgIndex;
use streamium_core::model::{Channel, MediaKind, Source};
use streamium_core::xtream::models::{self, AccountInfo, Category, LiveStream, VodStream};
use streamium_core::xtream::{Endpoints, LiveContainer};
use streamium_core::{m3u, xmltv};

use crate::config::SourceEntry;
use crate::net::{self, NetError};

/// Guides are the one download that can reach hundreds of megabytes.
const EPG_LIMIT: u64 = 512 * 1024 * 1024;
/// Playlists and API responses are large but not unbounded.
const TEXT_LIMIT: u64 = 256 * 1024 * 1024;
/// How deep a local library scan descends.
const SCAN_DEPTH: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct LoadStats {
    pub channels: usize,
    pub groups: usize,
    pub epg_channels: usize,
    pub programmes: usize,
    /// Wall-clock time for the whole load.
    pub elapsed_ms: u128,
}

#[derive(Debug)]
pub struct Loaded {
    pub catalog: Catalog,
    pub epg: EpgIndex,
    pub account: Option<AccountInfo>,
    /// Non-fatal problems worth showing: a guide that would not download, a
    /// playlist line that made no sense.
    pub warnings: Vec<String>,
    pub stats: LoadStats,
}

pub enum LoadMsg {
    /// What the worker is doing now, for the status line.
    Stage(String),
    Done(Box<Loaded>),
    Failed(String),
}

/// Load `entry` and report progress through `tx`. Runs on a worker thread.
pub fn load(entry: &SourceEntry, user_agent: &str, prefer_hls: bool, tx: &Sender<LoadMsg>) {
    let started = Instant::now();
    let stage = |s: &str| {
        let _ = tx.send(LoadMsg::Stage(s.to_string()));
    };

    let result = match &entry.source {
        Source::Xtream {
            base_url,
            username,
            password,
            ..
        } => load_xtream(
            base_url, username, password, entry, user_agent, prefer_hls, &stage,
        ),
        Source::Playlist { url, .. } => load_playlist(url, entry, user_agent, &stage),
        Source::LocalLibrary { path, .. } => load_local(path, &stage),
        Source::MediaServer { server_kind, .. } => Err(format!(
            "{server_kind} servers are not supported by the desktop shell yet"
        )),
    };

    match result {
        Ok(mut loaded) => {
            for id in &entry.favourites {
                loaded.catalog.set_favourite(id, true);
            }
            loaded.stats.channels = loaded.catalog.len();
            loaded.stats.groups = loaded.catalog.groups().len();
            loaded.stats.epg_channels = loaded.epg.channel_count();
            loaded.stats.programmes = loaded.epg.programme_count();
            loaded.stats.elapsed_ms = started.elapsed().as_millis();
            let _ = tx.send(LoadMsg::Done(Box::new(loaded)));
        }
        Err(e) => {
            let _ = tx.send(LoadMsg::Failed(e));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_xtream(
    base_url: &str,
    username: &str,
    password: &str,
    entry: &SourceEntry,
    user_agent: &str,
    prefer_hls: bool,
    stage: &dyn Fn(&str),
) -> Result<Loaded, String> {
    let endpoints = Endpoints::new(base_url, username, password).map_err(|e| e.to_string())?;
    let agent = net::agent(user_agent);
    let mut warnings = Vec::new();

    stage("Signing in…");
    let info_json = net::get_text(&agent, &endpoints.account_info(), TEXT_LIMIT)
        .map_err(|e| describe(e, "sign in"))?;
    let account: Option<AccountInfo> =
        models::decode_optional(&info_json).map_err(|e| format!("the server's reply to the sign-in request was not valid JSON ({e}). Check the server address."))?;
    match &account {
        Some(a) if !a.user_info.is_active() => {
            let status = a.user_info.status.as_deref().unwrap_or("not active");
            return Err(format!("the provider reports this account as {status}."));
        }
        Some(_) => {}
        None => warnings.push(
            "The server did not return account details; continuing without them.".to_string(),
        ),
    }

    stage("Fetching categories…");
    let cats_json = net::get_text(&agent, &endpoints.live_categories(), TEXT_LIMIT)
        .map_err(|e| describe(e, "list categories"))?;
    let categories: Vec<Category> = models::decode(&cats_json).unwrap_or_else(|e| {
        warnings.push(format!("Live categories could not be read ({e})."));
        Vec::new()
    });

    stage("Fetching channels…");
    let live_json = net::get_text(&agent, &endpoints.live_streams(None), TEXT_LIMIT)
        .map_err(|e| describe(e, "list channels"))?;
    let live: Vec<LiveStream> = models::decode(&live_json)
        .map_err(|e| format!("the channel list could not be read ({e})."))?;

    let container = if prefer_hls {
        LiveContainer::Hls
    } else {
        LiveContainer::Ts
    };
    let mut catalog = Catalog::from_xtream_live(&endpoints, &categories, &live, container);

    if entry.include_vod {
        stage("Fetching movies…");
        let vod_cats: Vec<Category> =
            net::get_text(&agent, &endpoints.vod_categories(), TEXT_LIMIT)
                .ok()
                .and_then(|j| models::decode(&j).ok())
                .unwrap_or_default();
        match net::get_text(&agent, &endpoints.vod_streams(None), TEXT_LIMIT) {
            Ok(j) => match models::decode::<Vec<VodStream>>(&j) {
                Ok(vod) => catalog.extend(Catalog::from_xtream_vod(&endpoints, &vod_cats, &vod)),
                Err(e) => warnings.push(format!("The movie list could not be read ({e}).")),
            },
            Err(e) => warnings.push(format!("Movies could not be fetched: {e}")),
        }
    }

    stage("Downloading the guide…");
    let epg_url = entry
        .epg_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| endpoints.xmltv());
    let epg = match fetch_epg(&agent, &epg_url) {
        Ok(index) => index,
        Err(e) => {
            warnings.push(format!("The guide could not be loaded: {e}"));
            EpgIndex::new()
        }
    };

    Ok(Loaded {
        catalog,
        epg,
        account,
        warnings,
        stats: LoadStats::default(),
    })
}

fn load_playlist(
    url: &str,
    entry: &SourceEntry,
    user_agent: &str,
    stage: &dyn Fn(&str),
) -> Result<Loaded, String> {
    let agent = net::agent(user_agent);
    let mut warnings = Vec::new();

    stage("Downloading the playlist…");
    let text = if is_local(url) {
        std::fs::read_to_string(local_path(url))
            .map_err(|e| format!("the playlist file could not be read: {e}"))?
    } else {
        net::get_text(&agent, url, TEXT_LIMIT).map_err(|e| describe(e, "download the playlist"))?
    };

    stage("Parsing…");
    let playlist = m3u::parse(&text).map_err(|e| e.to_string())?;
    if !playlist.warnings.is_empty() {
        warnings.push(format!(
            "{} line(s) in the playlist were not understood and were skipped.",
            playlist.warnings.len()
        ));
    }
    let catalog = Catalog::from_playlist(&playlist);

    let epg_url = entry
        .epg_url
        .clone()
        .filter(|u| !u.trim().is_empty())
        .or_else(|| playlist.epg_urls.first().cloned());
    let epg = match epg_url {
        Some(u) => {
            stage("Downloading the guide…");
            match fetch_epg(&agent, &u) {
                Ok(index) => index,
                Err(e) => {
                    warnings.push(format!("The guide could not be loaded: {e}"));
                    EpgIndex::new()
                }
            }
        }
        None => EpgIndex::new(),
    };

    Ok(Loaded {
        catalog,
        epg,
        account: None,
        warnings,
        stats: LoadStats::default(),
    })
}

/// Media extensions worth listing from a folder the user points at.
const MEDIA_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mov", "mkv", "avi", "webm", "flv", "wmv", "ts", "m2ts", "mts", "mpg", "mpeg",
    "mp3", "m4a", "flac", "ogg", "opus", "wav", "aac",
];

fn load_local(path: &str, stage: &dyn Fn(&str)) -> Result<Loaded, String> {
    stage("Scanning the folder…");
    let root = Path::new(path);
    if !root.is_dir() {
        return Err(format!("{path} is not a folder."));
    }
    let mut catalog = Catalog::new();
    let mut found = 0usize;
    scan(root, root, 0, &mut catalog, &mut found);
    if catalog.is_empty() {
        return Err(format!("no playable files were found under {path}."));
    }
    Ok(Loaded {
        catalog,
        epg: EpgIndex::new(),
        account: None,
        warnings: Vec::new(),
        stats: LoadStats::default(),
    })
}

fn scan(root: &Path, dir: &Path, depth: usize, catalog: &mut Catalog, found: &mut usize) {
    if depth > SCAN_DEPTH || *found > 50_000 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            scan(root, &path, depth + 1, catalog, found);
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if !MEDIA_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let url = url_for_file(&path);
        let mut channel = Channel::new(format!("file-{found}"), name, url);
        channel.kind = MediaKind::Personal;
        channel.group = path
            .parent()
            .filter(|p| *p != root)
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
        catalog.push(channel);
        *found += 1;
    }
}

/// Players take a plain path on every desktop platform, and it survives the
/// round trip through the settings file better than a percent-encoded URL.
fn url_for_file(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn is_local(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    !(lower.starts_with("http://") || lower.starts_with("https://"))
}

fn local_path(url: &str) -> String {
    url.trim()
        .strip_prefix("file://")
        .map(|p| p.trim_start_matches('/').to_string())
        .unwrap_or_else(|| url.trim().to_string())
}

/// Download and parse a guide, decompressing gzip on the fly so a 200 MB
/// XMLTV file never lands in memory whole.
fn fetch_epg(agent: &ureq::Agent, url: &str) -> Result<EpgIndex, String> {
    if is_local(url) {
        let bytes = std::fs::read(local_path(url)).map_err(|e| e.to_string())?;
        return xmltv::parse(&bytes).map_err(|e| e.to_string());
    }
    let reader = net::get_reader(agent, url, EPG_LIMIT).map_err(|e| e.to_string())?;
    let mut buffered = BufReader::with_capacity(1 << 16, reader);
    let gzipped = {
        let head = buffered.fill_buf().map_err(|e| e.to_string())?;
        head.len() >= 2 && head[0] == 0x1f && head[1] == 0x8b
    };
    if gzipped {
        let decoder = flate2::read::GzDecoder::new(buffered);
        xmltv::parse_reader(BufReader::with_capacity(1 << 16, decoder)).map_err(|e| e.to_string())
    } else {
        xmltv::parse_reader(buffered).map_err(|e| e.to_string())
    }
}

fn describe(e: NetError, what: &str) -> String {
    format!("Could not {what}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SourceEntry;
    use std::sync::mpsc::channel;

    /// A scratch directory that cleans up after itself.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let dir = std::env::temp_dir().join(format!(
                "streamium-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch dir");
            Scratch(dir)
        }

        fn write(&self, name: &str, contents: &str) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, contents).expect("write fixture");
            path.to_string_lossy().to_string()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const PLAYLIST: &str = "#EXTM3U url-tvg=\"http://example.com/guide.xml\"\n\
#EXTINF:-1 tvg-id=\"one.tv\" tvg-chno=\"101\" group-title=\"News\",One HD\n\
http://example.com/live/u/p/1\n\
#EXTINF:-1 tvg-id=\"two.tv\" group-title=\"Sport\",Two HD\n\
http://example.com/live/u/p/2.ts\n";

    const GUIDE: &str = "<?xml version=\"1.0\"?>\n\
<tv>\n\
<channel id=\"one.tv\"><display-name>One HD</display-name></channel>\n\
<programme start=\"20300101120000 +0000\" stop=\"20300101130000 +0000\" channel=\"one.tv\">\n\
<title>The News</title></programme>\n\
</tv>\n";

    fn run(entry: &SourceEntry) -> Result<Loaded, String> {
        let (tx, rx) = channel();
        load(entry, "test-agent", false, &tx);
        drop(tx);
        let mut stages = Vec::new();
        while let Ok(msg) = rx.recv() {
            match msg {
                LoadMsg::Stage(s) => stages.push(s),
                LoadMsg::Done(loaded) => {
                    assert!(!stages.is_empty(), "the load reported no progress");
                    return Ok(*loaded);
                }
                LoadMsg::Failed(e) => return Err(e),
            }
        }
        Err("the loader sent no result".to_string())
    }

    #[test]
    fn loads_a_local_playlist_with_its_guide() {
        let scratch = Scratch::new("playlist");
        let playlist = scratch.write("channels.m3u", PLAYLIST);
        let guide = scratch.write("guide.xml", GUIDE);

        let mut entry = SourceEntry::new(Source::Playlist {
            name: "Test".into(),
            url: playlist,
            epg_url: None,
        });
        entry.epg_url = Some(guide);
        entry.favourites = vec!["1".to_string()];

        let loaded = run(&entry).expect("load succeeds");
        assert_eq!(loaded.stats.channels, 2);
        assert_eq!(loaded.stats.groups, 2);
        assert_eq!(loaded.stats.programmes, 1);
        assert_eq!(loaded.catalog.channels()[0].name, "One HD");
        assert_eq!(loaded.catalog.channels()[0].number, Some(101));
        assert_eq!(loaded.catalog.channels()[0].group.as_deref(), Some("News"));
        // The guide joins on tvg-id.
        let epg_id = loaded
            .epg
            .resolve(loaded.catalog.channels()[0].epg_id.as_deref(), "One HD");
        assert_eq!(epg_id.as_deref(), Some("one.tv"));
        // Favourites stored in the settings file are restored onto the catalogue.
        assert!(loaded.catalog.is_favourite("1"));
    }

    #[test]
    fn reports_a_missing_playlist_instead_of_panicking() {
        let entry = SourceEntry::new(Source::Playlist {
            name: "Gone".into(),
            url: "/nonexistent/streamium/playlist.m3u".into(),
            epg_url: None,
        });
        let error = run(&entry).expect_err("a missing file is an error");
        assert!(error.contains("could not be read"), "{error}");
    }

    #[test]
    fn a_broken_guide_is_a_warning_not_a_failure() {
        let scratch = Scratch::new("badguide");
        let playlist = scratch.write("channels.m3u", PLAYLIST);

        let mut entry = SourceEntry::new(Source::Playlist {
            name: "Test".into(),
            url: playlist,
            epg_url: None,
        });
        entry.epg_url = Some("/nonexistent/streamium/guide.xml".to_string());

        let loaded = run(&entry).expect("the channels still load");
        assert_eq!(loaded.stats.channels, 2);
        assert_eq!(loaded.stats.programmes, 0);
        assert!(
            loaded.warnings.iter().any(|w| w.contains("guide")),
            "{:?}",
            loaded.warnings
        );
    }

    #[test]
    fn scans_a_folder_of_media() {
        let scratch = Scratch::new("folder");
        scratch.write("Film One.mkv", "not really a film");
        scratch.write("notes.txt", "ignored");
        std::fs::create_dir_all(scratch.0.join("Season 1")).expect("subdir");
        scratch.write("Season 1/Episode 1.mp4", "not really an episode");

        let entry = SourceEntry::new(Source::LocalLibrary {
            name: "Mine".into(),
            path: scratch.0.to_string_lossy().to_string(),
        });
        let loaded = run(&entry).expect("the folder scans");
        assert_eq!(loaded.stats.channels, 2, "text files must be skipped");
        let names: Vec<&str> = loaded
            .catalog
            .channels()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"Film One"), "{names:?}");
        assert!(names.contains(&"Episode 1"), "{names:?}");
        // Sub-folders become groups so a library is navigable.
        assert!(loaded
            .catalog
            .channels()
            .iter()
            .any(|c| c.group.as_deref() == Some("Season 1")));
    }

    #[test]
    fn an_empty_folder_is_reported_clearly() {
        let scratch = Scratch::new("empty");
        let entry = SourceEntry::new(Source::LocalLibrary {
            name: "Mine".into(),
            path: scratch.0.to_string_lossy().to_string(),
        });
        let error = run(&entry).expect_err("nothing to list is an error");
        assert!(error.contains("no playable files"), "{error}");
    }
}
