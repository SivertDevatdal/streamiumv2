//! End-to-end demuxer tests driven by a tiny in-test transport stream muxer.

use streamium_mpegts::{AccessUnit, Codec, Demuxer, Event, PACKET_SIZE};

// ---------- a minimal TS muxer for tests ----------

const VIDEO_PID: u16 = 0x100;
const AUDIO_PID: u16 = 0x101;
const PMT_PID: u16 = 0x1000;

fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn section(table_id: u8, id_ext: u16, version: u8, body: &[u8]) -> Vec<u8> {
    let mut s = vec![table_id];
    let len = 5 + body.len() + 4;
    s.push(0xB0 | ((len >> 8) as u8 & 0x0F));
    s.push(len as u8);
    s.extend_from_slice(&id_ext.to_be_bytes());
    s.push(0xC0 | (version << 1) | 1);
    s.push(0); // section_number
    s.push(0); // last_section_number
    s.extend_from_slice(body);
    let crc = crc32_mpeg(&s);
    s.extend_from_slice(&crc.to_be_bytes());
    s
}

fn pat_section(version: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&1u16.to_be_bytes());
    body.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
    section(0x00, 1, version, &body)
}

fn pmt_section(version: u8, video_type: u8, audio_type: u8) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes()); // PCR PID
    body.extend_from_slice(&0xF000u16.to_be_bytes()); // program_info_length = 0
    body.push(video_type);
    body.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes());
    body.extend_from_slice(&0xF000u16.to_be_bytes());
    body.push(audio_type);
    body.extend_from_slice(&(0xE000 | AUDIO_PID).to_be_bytes());
    // ES info: ISO 639 language descriptor "eng"
    let lang = [0x0A, 0x04, b'e', b'n', b'g', 0x00];
    body.extend_from_slice(&(0xF000u16 | lang.len() as u16).to_be_bytes());
    body.extend_from_slice(&lang);
    section(0x02, 1, version, &body)
}

fn timestamp_bytes(prefix: u8, ts: u64) -> [u8; 5] {
    [
        prefix | (((ts >> 30) & 0x07) as u8) << 1 | 1,
        ((ts >> 22) & 0xFF) as u8,
        (((ts >> 15) & 0x7F) as u8) << 1 | 1,
        ((ts >> 7) & 0xFF) as u8,
        ((ts & 0x7F) as u8) << 1 | 1,
    ]
}

/// Build a PES packet. `bounded` writes the real length (audio style);
/// otherwise the length field is 0 (video style, unbounded).
fn pes(stream_id: u8, pts: u64, dts: Option<u64>, payload: &[u8], bounded: bool) -> Vec<u8> {
    let mut header = Vec::new();
    let (flags, hdr_len) = match dts {
        Some(_) => (0xC0u8, 10u8),
        None => (0x80u8, 5u8),
    };
    header.push(0x80); // marker bits
    header.push(flags);
    header.push(hdr_len);
    match dts {
        Some(d) => {
            header.extend_from_slice(&timestamp_bytes(0x30, pts));
            header.extend_from_slice(&timestamp_bytes(0x10, d));
        }
        None => header.extend_from_slice(&timestamp_bytes(0x20, pts)),
    }
    let mut p = vec![0, 0, 1, stream_id];
    let len = if bounded {
        header.len() + payload.len()
    } else {
        0
    };
    p.extend_from_slice(&(len as u16).to_be_bytes());
    p.extend_from_slice(&header);
    p.extend_from_slice(payload);
    p
}

struct Muxer {
    out: Vec<u8>,
    cc: std::collections::HashMap<u16, u8>,
}

impl Muxer {
    fn new() -> Self {
        Muxer {
            out: Vec::new(),
            cc: Default::default(),
        }
    }

    fn next_cc(&mut self, pid: u16) -> u8 {
        let e = self.cc.entry(pid).or_insert(15);
        *e = (*e + 1) & 0x0F;
        *e
    }

    /// Emit `data` as one or more packets on `pid`, padding the last packet
    /// with an adaptation field. `psi` prepends the pointer field.
    fn write(&mut self, pid: u16, data: &[u8], psi: bool, random_access: bool) {
        let mut data = if psi {
            [&[0u8][..], data].concat()
        } else {
            data.to_vec()
        };
        let mut first = true;
        while !data.is_empty() || first {
            let cc = self.next_cc(pid);
            let pusi = first;
            let mut pkt = vec![
                0x47,
                (if pusi { 0x40 } else { 0 }) | ((pid >> 8) as u8 & 0x1F),
                pid as u8,
            ];
            let want_flags = first && random_access;
            let mut take = data.len().min(184);
            if want_flags {
                take = take.min(182); // room for the adaptation length byte + flags byte
            }
            if take < 184 {
                let af_total = 184 - take; // bytes of adaptation field incl. its length byte
                pkt.push(0x30 | cc);
                pkt.push((af_total - 1) as u8);
                if af_total >= 2 {
                    pkt.push(if want_flags { 0x40 } else { 0x00 });
                    pkt.extend(std::iter::repeat_n(0xFF, af_total - 2));
                }
            } else {
                pkt.push(0x10 | cc);
            }
            pkt.extend_from_slice(&data[..take]);
            data.drain(..take);
            assert_eq!(pkt.len(), PACKET_SIZE, "packet size");
            self.out.extend_from_slice(&pkt);
            first = false;
        }
    }

    fn psi(&mut self) {
        let pat = pat_section(0);
        self.write(0, &pat, true, false);
        let pmt = pmt_section(0, 0x1B, 0x0F);
        self.write(PMT_PID, &pmt, true, false);
    }

    fn null_packet(&mut self) {
        let mut pkt = vec![0x47, 0x1F, 0xFF, 0x10];
        pkt.extend(std::iter::repeat_n(0xFF, 184));
        self.out.extend_from_slice(&pkt);
    }
}

fn idr_frame(size: usize) -> Vec<u8> {
    let mut v = vec![
        0, 0, 0, 1, 0x67, 0x42, 0, 0, 0, 1, 0x68, 0xCE, 0, 0, 0, 1, 0x65, 0x88,
    ];
    v.extend((0..size).map(|i| (i % 251) as u8 + 1));
    v
}

fn p_frame(size: usize) -> Vec<u8> {
    let mut v = vec![0, 0, 0, 1, 0x41, 0x9A];
    v.extend((0..size).map(|i| (i % 251) as u8 + 1));
    v
}

fn adts_frame(size: usize) -> Vec<u8> {
    let mut v = vec![0xFF, 0xF1, 0x50, 0x80, 0, 0x1F, 0xFC];
    v.extend((0..size).map(|i| (i % 7) as u8));
    v
}

fn access_units(events: &[Event]) -> Vec<&AccessUnit> {
    events
        .iter()
        .filter_map(|e| {
            if let Event::AccessUnit(a) = e {
                Some(a)
            } else {
                None
            }
        })
        .collect()
}

// ---------- tests ----------

#[test]
fn discovers_program_and_streams() {
    let mut m = Muxer::new();
    m.psi();
    let mut d = Demuxer::new();
    d.push(&m.out);
    let events = d.events();
    let programs: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, Event::ProgramFound(_)))
        .collect();
    assert_eq!(programs.len(), 1);
    let Event::ProgramFound(p) = programs[0] else {
        unreachable!()
    };
    assert_eq!(p.number, 1);
    assert_eq!(p.pmt_pid, PMT_PID);
    assert_eq!(p.pcr_pid, VIDEO_PID);
    assert_eq!(p.streams.len(), 2);
    assert_eq!(p.streams[0].codec, Codec::H264);
    assert_eq!(p.streams[1].codec, Codec::AacAdts);
    assert_eq!(p.streams[1].language.as_deref(), Some("eng"));
    assert!(!events.iter().any(|e| matches!(e, Event::CrcError { .. })));
}

#[test]
fn assembles_video_and_audio_access_units_with_timing() {
    let mut m = Muxer::new();
    m.psi();
    let v0 = idr_frame(3000);
    let v1 = p_frame(700);
    let a0 = adts_frame(300);
    m.write(
        VIDEO_PID,
        &pes(0xE0, 90_000, Some(87_000), &v0, false),
        false,
        true,
    );
    m.write(AUDIO_PID, &pes(0xC0, 90_100, None, &a0, true), false, false);
    m.write(
        VIDEO_PID,
        &pes(0xE0, 93_600, None, &v1, false),
        false,
        false,
    );
    m.null_packet();
    // A final PSI repeat acts as the PUSI that flushes the unbounded video PES.
    m.psi();

    let mut d = Demuxer::new();
    d.push(&m.out);
    d.flush();
    let events = d.events();
    let aus = access_units(&events);
    assert_eq!(aus.len(), 3, "events: {events:?}");

    let video: Vec<_> = aus.iter().filter(|a| a.pid == VIDEO_PID).collect();
    assert_eq!(video.len(), 2);
    assert_eq!(video[0].codec, Codec::H264);
    assert_eq!(video[0].pts, Some(90_000));
    assert_eq!(video[0].dts, Some(87_000));
    assert!(video[0].random_access);
    assert_eq!(video[0].data, v0);
    assert_eq!(video[1].pts, Some(93_600));
    assert_eq!(video[1].dts, None);
    assert!(!video[1].random_access);
    assert_eq!(video[1].data, v1);

    let audio: Vec<_> = aus.iter().filter(|a| a.pid == AUDIO_PID).collect();
    assert_eq!(audio.len(), 1);
    assert_eq!(audio[0].codec, Codec::AacAdts);
    assert_eq!(audio[0].pts, Some(90_100));
    assert_eq!(audio[0].data, a0);

    // Audio PES packets are bounded, so the audio unit is emitted the moment
    // its last byte arrives. The unbounded video PES can only be closed by the
    // next start indicator on its PID, so it lands after the audio.
    let order: Vec<u16> = aus.iter().map(|a| a.pid).collect();
    assert_eq!(order, vec![AUDIO_PID, VIDEO_PID, VIDEO_PID]);
    assert_eq!(d.packets_seen() as usize, m.out.len() / PACKET_SIZE);
}

#[test]
fn byte_at_a_time_feeding_matches_bulk() {
    let mut m = Muxer::new();
    m.psi();
    m.write(
        VIDEO_PID,
        &pes(0xE0, 1000, None, &idr_frame(2500), false),
        false,
        true,
    );
    m.write(
        AUDIO_PID,
        &pes(0xC0, 1000, None, &adts_frame(200), true),
        false,
        false,
    );
    m.psi();

    let mut bulk = Demuxer::new();
    bulk.push(&m.out);
    bulk.flush();
    let bulk_events = bulk.events();

    let mut trickle = Demuxer::new();
    for chunk in m.out.chunks(1) {
        trickle.push(chunk);
    }
    trickle.flush();
    let trickle_events = trickle.events();

    assert_eq!(bulk_events, trickle_events);
    assert_eq!(access_units(&bulk_events).len(), 2);

    let mut odd = Demuxer::new();
    for chunk in m.out.chunks(1000) {
        odd.push(chunk);
    }
    odd.flush();
    assert_eq!(odd.events(), bulk_events);
}

#[test]
fn resyncs_after_garbage_and_truncated_packets() {
    let mut m = Muxer::new();
    m.psi();
    m.write(
        VIDEO_PID,
        &pes(0xE0, 5, None, &idr_frame(400), false),
        false,
        true,
    );
    m.psi();

    let mut corrupted = vec![0x00, 0x11, 0x22, 0x47, 0x33]; // garbage incl. a stray sync byte
    corrupted.extend_from_slice(&m.out[..PACKET_SIZE * 2]);
    corrupted.extend_from_slice(&m.out[PACKET_SIZE * 2 + 50..]); // drop 50 bytes mid-stream

    let mut d = Demuxer::new();
    d.push(&corrupted);
    d.flush();
    let events = d.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::SyncLost { .. })),
        "{events:?}"
    );
    // The program is still discovered from the repeated PSI.
    assert!(events.iter().any(|e| matches!(e, Event::ProgramFound(_))));
}

#[test]
fn continuity_gap_is_reported_and_partial_pes_dropped() {
    let mut m = Muxer::new();
    m.psi();
    m.write(
        VIDEO_PID,
        &pes(0xE0, 5, None, &idr_frame(2000), false),
        false,
        true,
    );
    m.psi();
    // Remove the second video packet (index: PAT=0, PMT=1, video starts at 2).
    let mut bytes = m.out.clone();
    bytes.drain(PACKET_SIZE * 3..PACKET_SIZE * 4);

    let mut d = Demuxer::new();
    d.push(&bytes);
    d.flush();
    let events = d.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::Discontinuity { pid } if *pid == VIDEO_PID)));
    assert!(
        access_units(&events).is_empty(),
        "a torn access unit must not reach the decoder"
    );
}

#[test]
fn video_before_first_keyframe_is_dropped() {
    let mut m = Muxer::new();
    m.psi();
    m.write(
        VIDEO_PID,
        &pes(0xE0, 1, None, &p_frame(500), false),
        false,
        false,
    );
    m.write(
        VIDEO_PID,
        &pes(0xE0, 2, None, &p_frame(500), false),
        false,
        false,
    );
    m.write(
        VIDEO_PID,
        &pes(0xE0, 3, None, &idr_frame(500), false),
        false,
        false,
    ); // no RAI flag, detected by NAL type
    m.write(
        VIDEO_PID,
        &pes(0xE0, 4, None, &p_frame(500), false),
        false,
        false,
    );
    m.psi();

    let mut d = Demuxer::new();
    d.push(&m.out);
    d.flush();
    let events = d.events();
    let pts: Vec<_> = access_units(&events)
        .iter()
        .map(|a| a.pts.unwrap())
        .collect();
    assert_eq!(pts, vec![3, 4]);
}

#[test]
fn corrupt_psi_is_ignored() {
    let mut m = Muxer::new();
    let mut bad_pat = pat_section(0);
    let n = bad_pat.len();
    bad_pat[n - 1] ^= 0xFF; // break the CRC
    m.write(0, &bad_pat, true, false);
    let pmt = pmt_section(0, 0x1B, 0x0F);
    m.write(PMT_PID, &pmt, true, false);

    let mut d = Demuxer::new();
    d.push(&m.out);
    let events = d.events();
    assert!(events
        .iter()
        .any(|e| matches!(e, Event::CrcError { pid: 0 })));
    assert!(!events.iter().any(|e| matches!(e, Event::ProgramFound(_))));
}

#[test]
fn pmt_version_change_replaces_program() {
    let mut m = Muxer::new();
    m.psi();
    let pmt_v1 = pmt_section(1, 0x24, 0x81); // HEVC + AC-3
    m.write(PMT_PID, &pmt_v1, true, false);
    m.write(PMT_PID, &pmt_v1, true, false); // repeat must not re-announce

    let mut d = Demuxer::new();
    d.push(&m.out);
    let events = d.events();
    let found: Vec<_> = events
        .iter()
        .filter_map(|e| {
            if let Event::ProgramFound(p) = e {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(found.len(), 2);
    assert_eq!(found[1].streams[0].codec, Codec::H265);
    assert_eq!(found[1].streams[1].codec, Codec::Ac3);
    assert_eq!(d.programs().count(), 1);
}

#[test]
fn pcr_is_extracted() {
    // Hand-build a packet with an adaptation field carrying PCR base=90000, ext=7.
    let mut m = Muxer::new();
    m.psi();
    let base: u64 = 90_000;
    let ext: u64 = 7;
    let mut pkt = vec![0x47, 0x01, 0x00, 0x20]; // PID 0x100, adaptation only
    pkt.push(183);
    pkt.push(0x10); // PCR flag
    pkt.push((base >> 25) as u8);
    pkt.push((base >> 17) as u8);
    pkt.push((base >> 9) as u8);
    pkt.push((base >> 1) as u8);
    pkt.push(((base & 1) as u8) << 7 | 0x7E | ((ext >> 8) as u8 & 1));
    pkt.push(ext as u8);
    pkt.extend(std::iter::repeat_n(0xFF, PACKET_SIZE - pkt.len()));
    m.out.extend_from_slice(&pkt);

    let mut d = Demuxer::new();
    d.push(&m.out);
    let events = d.events();
    assert!(events.contains(&Event::Pcr {
        pid: VIDEO_PID,
        pcr: base * 300 + ext
    }));
}
