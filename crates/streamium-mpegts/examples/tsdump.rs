//! Demux a transport stream file and report what was found, optionally
//! writing each elementary stream to disk so an external decoder can verify
//! that the output is valid.
//!
//!     cargo run -p streamium-mpegts --example tsdump -- input.ts [out_dir] [chunk_size]
//!
//! The chunk size controls how the file is fed to the demuxer, which is how
//! the "arbitrary network chunk" behaviour is exercised from the command line.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::PathBuf;

use streamium_mpegts::{Codec, Demuxer, Event};

fn extension(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::H265 => "hevc",
        Codec::Mpeg2Video => "m2v",
        Codec::AacAdts => "aac",
        Codec::AacLatm => "latm",
        Codec::MpegAudio => "mp2",
        Codec::Ac3 => "ac3",
        Codec::Eac3 => "eac3",
        Codec::Dts => "dts",
        Codec::DvbSubtitle => "dvbsub",
        Codec::Teletext => "ttx",
        Codec::Other(_) => "bin",
    }
}

#[derive(Default)]
struct PidStats {
    codec: Option<Codec>,
    units: u64,
    bytes: u64,
    keyframes: u64,
    first_pts: Option<u64>,
    last_pts: Option<u64>,
    missing_pts: u64,
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(input) = args.next() else {
        eprintln!("usage: tsdump <input.ts> [out_dir] [chunk_size]");
        std::process::exit(2);
    };
    let out_dir = args.next().map(PathBuf::from);
    let chunk_size: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(32 * 1024);

    let file = File::open(&input).unwrap_or_else(|e| {
        eprintln!("cannot open {input}: {e}");
        std::process::exit(1);
    });
    let mut reader = BufReader::new(file);
    let mut demuxer = Demuxer::new();
    let mut buf = vec![0u8; chunk_size.max(1)];

    let mut stats: BTreeMap<u16, PidStats> = BTreeMap::new();
    let mut writers: BTreeMap<u16, File> = BTreeMap::new();
    let mut programs = 0u32;
    let mut discontinuities = 0u32;
    let mut sync_lost = 0u32;
    let mut crc_errors = 0u32;
    let mut pcr_samples = 0u64;

    if let Some(dir) = &out_dir {
        std::fs::create_dir_all(dir).expect("create output directory");
    }

    let mut handle = |events: Vec<Event>,
                      stats: &mut BTreeMap<u16, PidStats>,
                      writers: &mut BTreeMap<u16, File>| {
        for event in events {
            match event {
                Event::ProgramFound(p) => {
                    programs += 1;
                    println!("program {} (pcr pid 0x{:04x})", p.number, p.pcr_pid);
                    for s in &p.streams {
                        println!(
                            "  pid 0x{:04x}  {:?}  stream_type 0x{:02x}  lang {}",
                            s.pid,
                            s.codec,
                            s.stream_type,
                            s.language.as_deref().unwrap_or("-")
                        );
                        stats.entry(s.pid).or_default().codec = Some(s.codec);
                        if let Some(dir) = &out_dir {
                            let path = dir.join(format!("{:04x}.{}", s.pid, extension(s.codec)));
                            writers.entry(s.pid).or_insert_with(|| {
                                File::create(&path).expect("create elementary stream file")
                            });
                        }
                    }
                }
                Event::AccessUnit(au) => {
                    let entry = stats.entry(au.pid).or_default();
                    entry.codec.get_or_insert(au.codec);
                    entry.units += 1;
                    entry.bytes += au.data.len() as u64;
                    if au.random_access {
                        entry.keyframes += 1;
                    }
                    match au.pts {
                        Some(pts) => {
                            entry.first_pts.get_or_insert(pts);
                            entry.last_pts = Some(pts);
                        }
                        None => entry.missing_pts += 1,
                    }
                    if let Some(w) = writers.get_mut(&au.pid) {
                        w.write_all(&au.data).expect("write elementary stream");
                    }
                }
                Event::Pcr { .. } => pcr_samples += 1,
                Event::Discontinuity { pid } => {
                    discontinuities += 1;
                    eprintln!("discontinuity on pid 0x{pid:04x}");
                }
                Event::SyncLost { bytes } => {
                    sync_lost += 1;
                    eprintln!("sync lost, skipped {bytes} bytes");
                }
                Event::CrcError { pid } => {
                    crc_errors += 1;
                    eprintln!("crc error on pid 0x{pid:04x}");
                }
            }
        }
    };

    loop {
        let n = reader.read(&mut buf).expect("read input");
        if n == 0 {
            break;
        }
        demuxer.push(&buf[..n]);
        handle(demuxer.events(), &mut stats, &mut writers);
    }
    demuxer.flush();
    handle(demuxer.events(), &mut stats, &mut writers);

    println!("\npackets: {}", demuxer.packets_seen());
    println!("programs: {programs}  pcr samples: {pcr_samples}");
    println!(
        "discontinuities: {discontinuities}  sync losses: {sync_lost}  crc errors: {crc_errors}"
    );
    for (pid, s) in &stats {
        let span = match (s.first_pts, s.last_pts) {
            (Some(a), Some(b)) => format!("{:.3}s", (b.saturating_sub(a)) as f64 / 90_000.0),
            _ => "-".into(),
        };
        println!(
            "pid 0x{:04x}  {:?}  units {}  bytes {}  keyframes {}  pts span {}  missing pts {}",
            pid,
            s.codec.unwrap_or(Codec::Other(0)),
            s.units,
            s.bytes,
            s.keyframes,
            span,
            s.missing_pts
        );
    }
}
