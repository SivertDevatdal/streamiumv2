//! A diagnostic for a provider, using exactly the code the app uses.
//!
//!     cargo run -p streamium-probe -- xtream http://host:8080 USER PASS
//!     cargo run -p streamium-probe -- playlist http://host/list.m3u
//!     cargo run -p streamium-probe -- stream http://host/live/u/p/1.ts
//!
//! Networking is delegated to `curl`, so this stays free of dependencies and
//! the core crates keep their sans-IO property. Passwords are masked in the
//! output, so it is safe to paste a report into a bug report.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Instant;

use streamium_core::catalog::Catalog;
use streamium_core::model::StreamFormat;
use streamium_core::xtream::models as xm;
use streamium_core::xtream::{Endpoints, LiveContainer};
use streamium_core::{m3u, xmltv};
use streamium_mpegts::{Codec, Demuxer, Event};

const RULE: &str = "────────────────────────────────────────────────────────────";

struct Secrets {
    values: Vec<String>,
}

impl Secrets {
    fn mask(&self, text: &str) -> String {
        let mut out = text.to_string();
        for secret in &self.values {
            if secret.len() >= 3 {
                out = out.replace(secret.as_str(), "••••••");
            }
        }
        out
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage:\n  \
        probe xtream <server> <username> <password>\n  \
        probe playlist <url> [epg-url]\n  \
        probe stream <url> [seconds]";

    match args.first().map(String::as_str) {
        Some("xtream") if args.len() >= 4 => {
            let secrets = Secrets {
                values: vec![args[3].clone(), args[2].clone()],
            };
            probe_xtream(&args[1], &args[2], &args[3], &secrets);
        }
        Some("playlist") if args.len() >= 2 => {
            let secrets = Secrets { values: vec![] };
            probe_playlist(&args[1], args.get(2).map(String::as_str), &secrets);
        }
        Some("stream") if args.len() >= 2 => {
            let seconds: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(8);
            probe_stream(&args[1], seconds, &Secrets { values: vec![] });
        }
        _ => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }
}

// ---------------------------------------------------------------------------
// HTTP, delegated to curl
// ---------------------------------------------------------------------------

struct Response {
    status: u16,
    content_type: String,
    body: Vec<u8>,
    seconds: f64,
}

fn fetch(url: &str, max_seconds: u32) -> Result<Response, String> {
    let file = std::env::temp_dir().join(format!("streamium-probe-{}", std::process::id()));
    let started = Instant::now();
    let output = Command::new("curl")
        .args([
            "-sS",
            "-L",
            "--max-time",
            &max_seconds.to_string(),
            "--connect-timeout",
            "10",
            "-A",
            "Streamium-Probe/1.0",
            "-o",
            file.to_str().ok_or("bad temp path")?,
            "-w",
            "%{http_code} %{content_type}",
            url,
        ])
        .output()
        .map_err(|e| format!("could not run curl: {e}"))?;

    let meta = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let body = std::fs::read(&file).unwrap_or_default();
    let _ = std::fs::remove_file(&file);

    let mut parts = meta.split_whitespace();
    let status: u16 = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let content_type = parts.next().unwrap_or("").to_string();
    let seconds = started.elapsed().as_secs_f64();

    // A live stream is cut off by --max-time; that is a success, not a failure,
    // as long as bytes arrived.
    if status == 0 && body.is_empty() {
        let err = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "no response".into()
        } else {
            err
        });
    }
    Ok(Response {
        status,
        content_type,
        body,
        seconds,
    })
}

fn report(label: &str, url: &str, secrets: &Secrets, max_seconds: u32) -> Option<Response> {
    println!("\n{label}");
    println!("  GET {}", secrets.mask(url));
    match fetch(url, max_seconds) {
        Ok(response) => {
            println!(
                "  {} {} · {} · {:.2}s",
                if (200..300).contains(&response.status) {
                    "ok"
                } else {
                    "FAILED"
                },
                response.status,
                human_bytes(response.body.len()),
                response.seconds
            );
            if !response.content_type.is_empty() {
                println!("  content-type: {}", response.content_type);
            }
            if !(200..300).contains(&response.status) {
                let preview = String::from_utf8_lossy(&response.body);
                let preview = preview.trim();
                if !preview.is_empty() {
                    println!("  body: {}", secrets.mask(&truncate(preview, 200)));
                }
                return None;
            }
            Some(response)
        }
        Err(e) => {
            println!("  FAILED: {}", secrets.mask(&e));
            None
        }
    }
}

/// Identify the containers that are not transport streams, so a progressive
/// file is never fed to the demuxer and reported as corrupt.
fn recognise_container(body: &[u8]) -> Option<&'static str> {
    if body.len() >= 12 && &body[4..8] == b"ftyp" {
        return Some("An MP4 or QuickTime file.");
    }
    if body.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some("A Matroska or WebM file.");
    }
    if body.starts_with(b"FLV") {
        return Some("A Flash video file.");
    }
    if body.starts_with(b"OggS") {
        return Some("An Ogg file.");
    }
    if body.starts_with(b"RIFF") {
        return Some("A RIFF file, usually AVI or WAV.");
    }
    if body.starts_with(b"ID3") || body.starts_with(b"fLaC") {
        return Some("An audio file.");
    }
    if body.starts_with(&[0x30, 0x26, 0xB2, 0x75]) {
        return Some("An ASF or WMV file.");
    }
    None
}

/// A transport stream has a 0x47 sync byte every 188 bytes. Requiring the
/// pattern to repeat avoids matching a stray 0x47 in some other format.
fn looks_like_transport_stream(body: &[u8]) -> bool {
    const PACKET: usize = 188;
    for offset in 0..PACKET.min(body.len()) {
        if body[offset] != 0x47 {
            continue;
        }
        let mut position = offset;
        let mut confirmed = 0;
        while position + PACKET < body.len() && confirmed < 4 {
            position += PACKET;
            if body[position] != 0x47 {
                break;
            }
            confirmed += 1;
        }
        if confirmed >= 2 {
            return true;
        }
    }
    false
}

fn human_bytes(n: usize) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = n as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{n} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_string()
    } else {
        text.chars().take(max).collect::<String>() + "…"
    }
}

// ---------------------------------------------------------------------------
// Xtream
// ---------------------------------------------------------------------------

fn probe_xtream(server: &str, username: &str, password: &str, secrets: &Secrets) {
    println!("{RULE}\nXtream provider check\n{RULE}");
    println!("server:   {server}");
    println!("username: {}", secrets.mask(username));

    let endpoints = match Endpoints::new(server, username, password) {
        Ok(e) => e,
        Err(e) => {
            println!("\nThe server address could not be used: {e}");
            println!("Give the part before /player_api.php, for example http://host:8080");
            return;
        }
    };
    println!("normalised base: {}", endpoints.base_url());

    // ---- account ----
    let Some(response) = report("1. Account", &endpoints.account_info(), secrets, 30) else {
        println!(
            "\nThe account call failed, so nothing else can work. Check the address and port."
        );
        return;
    };
    let text = String::from_utf8_lossy(&response.body);
    let account: xm::AccountInfo = match xm::decode_optional(&text) {
        Ok(Some(a)) => a,
        Ok(None) => {
            println!("  The server returned an empty response; usually wrong credentials.");
            return;
        }
        Err(e) => {
            println!("  Could not read the response as Xtream JSON: {e}");
            println!(
                "  First bytes: {}",
                secrets.mask(&truncate(text.trim(), 200))
            );
            return;
        }
    };
    let info = &account.user_info;
    println!(
        "  status:      {}",
        info.status.as_deref().unwrap_or("unknown")
    );
    println!("  active:      {}", info.is_active());
    println!(
        "  connections: {} of {}",
        info.active_cons
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into()),
        info.max_connections
            .map(|v| v.to_string())
            .unwrap_or_else(|| "?".into())
    );
    if let Some(expiry) = info.exp_date {
        let (y, m, d, ..) = xmltv::civil_from_unix(expiry);
        println!("  expires:     {y:04}-{m:02}-{d:02}");
    }
    println!("  trial:       {}", info.is_trial.unwrap_or(false));
    println!("  formats:     {:?}", info.allowed_output_formats);
    if let Some(tz) = &account.server_info.timezone {
        println!("  server tz:   {tz}");
    }

    if !info.is_active() {
        println!("\nThe provider reports this account as not usable. Stop here.");
        return;
    }

    // The app makes the same choice.
    let formats: Vec<String> = info
        .allowed_output_formats
        .iter()
        .map(|f| f.to_lowercase())
        .collect();
    let use_hls = !formats.is_empty() && !formats.contains(&"ts".to_string());
    let container = if use_hls {
        LiveContainer::Hls
    } else {
        LiveContainer::Ts
    };
    println!(
        "  the app would use: {}",
        if use_hls {
            "HLS (.m3u8)"
        } else {
            "transport streams (.ts)"
        }
    );

    // ---- categories and channels ----
    let categories: Vec<xm::Category> = report(
        "2. Live categories",
        &endpoints.live_categories(),
        secrets,
        60,
    )
    .and_then(|r| {
        xm::decode_optional(&String::from_utf8_lossy(&r.body))
            .ok()
            .flatten()
    })
    .unwrap_or_default();
    println!("  {} categories", categories.len());

    let Some(response) = report(
        "3. Live channels",
        &endpoints.live_streams(None),
        secrets,
        120,
    ) else {
        return;
    };
    let streams: Vec<xm::LiveStream> =
        match xm::decode_optional(&String::from_utf8_lossy(&response.body)) {
            Ok(Some(s)) => s,
            Ok(None) => Vec::new(),
            Err(e) => {
                println!("  Could not read the channel list: {e}");
                return;
            }
        };
    println!("  {} channels", streams.len());

    let catalog = Catalog::from_xtream_live(&endpoints, &categories, &streams, container);
    println!("  {} usable after parsing", catalog.len());
    println!("  {} groups", catalog.groups().len());
    let with_epg = catalog
        .channels()
        .iter()
        .filter(|c| c.epg_id.is_some())
        .count();
    println!("  {with_epg} carry a guide id");
    let with_catchup = catalog
        .channels()
        .iter()
        .filter(|c| c.catchup.is_some())
        .count();
    println!("  {with_catchup} offer catch-up");

    for channel in catalog.channels().iter().take(3) {
        println!(
            "    #{:<5} {:<34} {}",
            channel.number.map(|n| n.to_string()).unwrap_or_default(),
            truncate(&channel.name, 34),
            secrets.mask(&channel.url)
        );
    }

    // ---- VOD ----
    if let Some(response) = report("4. Movies", &endpoints.vod_streams(None), secrets, 120) {
        let vod: Vec<xm::VodStream> = xm::decode_optional(&String::from_utf8_lossy(&response.body))
            .ok()
            .flatten()
            .unwrap_or_default();
        println!("  {} movies", vod.len());
    }

    // ---- guide ----
    if let Some(response) = report("5. Guide (XMLTV)", &endpoints.xmltv(), secrets, 300) {
        match xmltv::parse(&response.body) {
            Ok(mut index) => {
                index.finish();
                println!(
                    "  {} channels, {} programmes",
                    index.channel_count(),
                    index.programme_count()
                );
                let matched = catalog
                    .channels()
                    .iter()
                    .filter(|c| index.resolve(c.epg_id.as_deref(), &c.name).is_some())
                    .count();
                println!(
                    "  {matched} of {} channels match a guide entry",
                    catalog.len()
                );
            }
            Err(e) => println!("  Could not parse the guide: {e}"),
        }
    }

    // ---- playback ----
    match catalog.channels().first() {
        Some(channel) => {
            println!("\n{RULE}\nPlayback check: {}\n{RULE}", channel.name);
            probe_stream(&channel.url, 8, secrets);
        }
        None => println!("\nNo channels to test playback with."),
    }
}

// ---------------------------------------------------------------------------
// Playlist
// ---------------------------------------------------------------------------

fn probe_playlist(url: &str, epg_url: Option<&str>, secrets: &Secrets) {
    println!("{RULE}\nPlaylist check\n{RULE}");

    let Some(response) = report("1. Playlist", url, secrets, 120) else {
        return;
    };
    let text = String::from_utf8_lossy(&response.body);
    let playlist = match m3u::parse(&text) {
        Ok(p) => p,
        Err(e) => {
            println!("  Not a playlist: {e}");
            println!("  First bytes: {}", truncate(text.trim(), 200));
            return;
        }
    };
    if playlist.kind == m3u::PlaylistKind::HlsMedia {
        println!("  This is a single HLS stream, not a channel list.");
        probe_stream(url, 8, secrets);
        return;
    }

    let catalog = Catalog::from_playlist(&playlist);
    println!(
        "  {} channels in {} groups",
        catalog.len(),
        catalog.groups().len()
    );
    println!("  guide URLs in the header: {:?}", playlist.epg_urls);
    if !playlist.warnings.is_empty() {
        println!(
            "  {} lines could not be parsed, first few:",
            playlist.warnings.len()
        );
        for (line, message) in playlist.warnings.iter().take(3) {
            println!("    line {line}: {message}");
        }
    }

    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    for channel in catalog.channels() {
        *formats.entry(format!("{:?}", channel.format)).or_default() += 1;
    }
    println!("  formats: {formats:?}");

    for channel in catalog.channels().iter().take(3) {
        println!(
            "    {:<34} {}",
            truncate(&channel.name, 34),
            secrets.mask(&channel.url)
        );
    }

    let guide = epg_url
        .map(String::from)
        .or_else(|| playlist.epg_urls.first().cloned());
    if let Some(guide) = guide {
        if let Some(response) = report("2. Guide (XMLTV)", &guide, secrets, 300) {
            match xmltv::parse(&response.body) {
                Ok(index) => {
                    println!(
                        "  {} channels, {} programmes",
                        index.channel_count(),
                        index.programme_count()
                    );
                    let matched = catalog
                        .channels()
                        .iter()
                        .filter(|c| index.resolve(c.epg_id.as_deref(), &c.name).is_some())
                        .count();
                    println!(
                        "  {matched} of {} channels match a guide entry",
                        catalog.len()
                    );
                }
                Err(e) => println!("  Could not parse the guide: {e}"),
            }
        }
    }

    if let Some(channel) = catalog.channels().first() {
        println!("\n{RULE}\nPlayback check: {}\n{RULE}", channel.name);
        probe_stream(&channel.url, 8, secrets);
    }
}

// ---------------------------------------------------------------------------
// Stream
// ---------------------------------------------------------------------------

fn probe_stream(url: &str, seconds: u32, secrets: &Secrets) {
    let format = StreamFormat::infer(url);
    println!("\nURL suggests: {format:?}");

    let Some(response) = report(&format!("Reading for {seconds}s"), url, secrets, seconds) else {
        return;
    };
    let body = &response.body;
    if body.is_empty() {
        println!("  No data arrived. The channel may be offline or the connection limit reached.");
        return;
    }
    let rate = body.len() as f64 * 8.0 / response.seconds / 1_000_000.0;
    println!("  about {rate:.1} Mbit/s");

    let head = String::from_utf8_lossy(&body[..body.len().min(200)]);
    if head.trim_start().starts_with("#EXTM3U") {
        println!("  This is an HLS playlist; AVPlayer handles it natively.");
        for line in head.lines().take(6) {
            println!("    {}", secrets.mask(line));
        }
        return;
    }
    if head.trim_start().starts_with('<') {
        println!("  The server returned a web page, not media:");
        println!("    {}", secrets.mask(&truncate(head.trim(), 200)));
        return;
    }

    if let Some(container) = recognise_container(body) {
        println!("  {container}");
        println!("  AVPlayer decodes this natively; the transport stream demuxer is not involved.");
        return;
    }
    if !looks_like_transport_stream(body) {
        println!(
            "  Unrecognised. First bytes: {:02x?}",
            &body[..body.len().min(16)]
        );
        return;
    }

    let mut demuxer = Demuxer::new();
    let mut programs = Vec::new();
    let mut units: BTreeMap<u16, (Codec, usize, usize)> = BTreeMap::new();
    let mut discontinuities = 0usize;
    let mut sync_losses = 0usize;

    for chunk in body.chunks(4096) {
        demuxer.push(chunk);
        for event in demuxer.events() {
            match event {
                Event::ProgramFound(p) => programs.push(p),
                Event::AccessUnit(au) => {
                    let entry = units.entry(au.pid).or_insert((au.codec, 0, 0));
                    entry.1 += 1;
                    if au.random_access {
                        entry.2 += 1;
                    }
                }
                Event::Discontinuity { .. } => discontinuities += 1,
                Event::SyncLost { .. } => sync_losses += 1,
                Event::CrcError { .. } | Event::Pcr { .. } => {}
            }
        }
    }
    demuxer.flush();
    for event in demuxer.events() {
        if let Event::AccessUnit(au) = event {
            let entry = units.entry(au.pid).or_insert((au.codec, 0, 0));
            entry.1 += 1;
        }
    }

    println!("  {} transport packets", demuxer.packets_seen());
    if sync_losses > 0 {
        println!("  {sync_losses} resynchronisations");
    }
    if discontinuities > 0 {
        println!("  {discontinuities} discontinuities (packet loss)");
    }

    if programs.is_empty() {
        println!("  No program table arrived in this sample; try a longer read:");
        println!("    probe stream <url> 20");
        return;
    }
    for program in &programs {
        println!("  program {}:", program.number);
        for stream in &program.streams {
            println!(
                "    pid 0x{:04x}  {:?}{}",
                stream.pid,
                stream.codec,
                stream
                    .language
                    .as_ref()
                    .map(|l| format!("  [{l}]"))
                    .unwrap_or_default()
            );
        }
    }
    for (pid, (codec, count, keyframes)) in &units {
        println!("  pid 0x{pid:04x}  {codec:?}  {count} access units, {keyframes} key frames");
    }

    let video_ok = units
        .values()
        .any(|(c, n, k)| c.is_video() && *n > 0 && *k > 0);
    let audio_ok = units.values().any(|(c, n, _)| c.is_audio() && *n > 0);
    println!();
    match (video_ok, audio_ok) {
        (true, true) => println!("  Video and audio both decoded. This channel will play."),
        (true, false) => println!("  Video only: no audio was recovered."),
        (false, true) => println!("  Audio only: no key frame arrived, so video cannot start yet."),
        (false, false) => println!("  Nothing playable was recovered from this sample."),
    }
}
