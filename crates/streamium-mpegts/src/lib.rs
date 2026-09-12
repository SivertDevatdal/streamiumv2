//! # streamium-mpegts
//!
//! A small, allocation-conscious MPEG-2 transport stream (ISO/IEC 13818-1)
//! demuxer. It turns a byte stream of 188-byte TS packets into
//! [`Event`]s: discovered programs, elementary-stream access units with
//! PTS/DTS, PCR clock samples and error notices.
//!
//! It does **not** decode video or audio. On Apple platforms the access units
//! go straight into VideoToolbox / `AVSampleBufferDisplayLayer` and
//! `AVSampleBufferAudioRenderer`; on Android into `MediaCodec`. Hardware
//! decoders are what make playback fast and cool; this crate exists so that
//! raw `.ts` IPTV streams can reach them without an ffmpeg dependency.
//!
//! Design notes:
//! * Resynchronises on the `0x47` sync byte after garbage or packet loss.
//! * Tracks continuity counters per PID and reports gaps as
//!   [`Event::Discontinuity`] so the renderer can flush cleanly.
//! * Verifies PSI CRC32; a corrupt PAT/PMT is ignored, not acted upon.
//! * Feeding is incremental: bytes may arrive in any chunk size.

mod crc;
mod pes;
mod psi;

use std::collections::HashMap;

pub use pes::TimingInfo;

pub const PACKET_SIZE: usize = 188;
const SYNC_BYTE: u8 = 0x47;
const PID_PAT: u16 = 0x0000;
const PID_NULL: u16 = 0x1FFF;

/// Codec of an elementary stream as signalled by the PMT.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Codec {
    H264,
    H265,
    Mpeg2Video,
    /// AAC in ADTS frames (stream type 0x0F).
    AacAdts,
    /// AAC in LATM/LOAS (stream type 0x11).
    AacLatm,
    /// MPEG-1/2 audio layer II/III.
    MpegAudio,
    Ac3,
    Eac3,
    Dts,
    /// DVB bitmap subtitles.
    DvbSubtitle,
    /// DVB teletext (often carries subtitles too).
    Teletext,
    /// A stream type we recognise as media but cannot describe further.
    Other(u8),
}

impl Codec {
    pub fn is_video(self) -> bool {
        matches!(self, Codec::H264 | Codec::H265 | Codec::Mpeg2Video)
    }
    pub fn is_audio(self) -> bool {
        matches!(
            self,
            Codec::AacAdts
                | Codec::AacLatm
                | Codec::MpegAudio
                | Codec::Ac3
                | Codec::Eac3
                | Codec::Dts
        )
    }
    pub fn is_subtitle(self) -> bool {
        matches!(self, Codec::DvbSubtitle | Codec::Teletext)
    }
}

/// One elementary stream inside a program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub pid: u16,
    pub codec: Codec,
    /// Raw `stream_type` from the PMT, for diagnostics.
    pub stream_type: u8,
    /// ISO 639 language code from the language descriptor, if present.
    pub language: Option<String>,
}

/// A program (service) as described by a PMT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    pub number: u16,
    pub pmt_pid: u16,
    pub pcr_pid: u16,
    pub streams: Vec<Stream>,
}

/// An access unit (one complete PES payload) ready for a decoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessUnit {
    pub pid: u16,
    pub codec: Codec,
    /// Presentation timestamp in 90 kHz ticks, if the PES carried one.
    pub pts: Option<u64>,
    /// Decode timestamp in 90 kHz ticks, if different from PTS.
    pub dts: Option<u64>,
    /// Set when the adaptation field flagged a random access point (key frame).
    pub random_access: bool,
    /// Elementary stream bytes. For H.264/H.265 this is Annex B (start codes);
    /// for AAC it is ADTS frames; for AC-3 it is sync frames.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The PAT/PMT chain has been parsed (or re-parsed after a version change).
    ProgramFound(Program),
    /// A complete access unit for one elementary stream.
    AccessUnit(AccessUnit),
    /// A program clock reference sample: `(pid, 27 MHz ticks)`.
    Pcr { pid: u16, pcr: u64 },
    /// A continuity-counter gap or a signalled discontinuity on `pid`.
    Discontinuity { pid: u16 },
    /// The demuxer lost sync and skipped `bytes` before finding it again.
    SyncLost { bytes: usize },
    /// A PSI table failed its CRC check.
    CrcError { pid: u16 },
}

#[derive(Debug, Default)]
struct PidState {
    last_cc: Option<u8>,
    pes: Option<pes::PesAssembler>,
    section: Option<psi::SectionAssembler>,
}

/// Incremental demuxer. Create one per stream; call [`Demuxer::push`] with
/// each chunk of bytes as it arrives, then drain [`Demuxer::events`].
#[derive(Debug, Default)]
pub struct Demuxer {
    pending: Vec<u8>,
    pids: HashMap<u16, PidState>,
    /// program number → PMT PID, from the PAT.
    pat: HashMap<u16, u16>,
    pat_version: Option<u8>,
    programs: HashMap<u16, Program>,
    pmt_versions: HashMap<u16, u8>,
    /// elementary PID → codec, for fast lookup while assembling PES.
    es_codecs: HashMap<u16, Codec>,
    events: Vec<Event>,
    packets_seen: u64,
    /// Video access units are emitted only from the first random-access point
    /// so decoders never start on a P-frame.
    seen_keyframe: HashMap<u16, bool>,
}

impl Demuxer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Packets processed so far (including null packets).
    pub fn packets_seen(&self) -> u64 {
        self.packets_seen
    }

    pub fn programs(&self) -> impl Iterator<Item = &Program> {
        self.programs.values()
    }

    /// Feed bytes. Any chunk size is fine; partial packets are retained.
    pub fn push(&mut self, bytes: &[u8]) {
        if self.pending.is_empty() {
            self.consume(bytes);
        } else {
            self.pending.extend_from_slice(bytes);
            let buf = std::mem::take(&mut self.pending);
            self.consume(&buf);
        }
    }

    /// Take all events produced so far, in order.
    pub fn events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Flush the partially assembled PES on every PID (call at end of stream).
    pub fn flush(&mut self) {
        let pids: Vec<u16> = self.pids.keys().copied().collect();
        for pid in pids {
            self.flush_pes(pid, false);
        }
    }

    /// Forget partial state after a seek or reconnect, keeping program info.
    pub fn reset_timing(&mut self) {
        self.pending.clear();
        for st in self.pids.values_mut() {
            st.last_cc = None;
            st.pes = None;
            st.section = None;
        }
        self.seen_keyframe.clear();
    }

    fn consume(&mut self, buf: &[u8]) {
        let mut pos = 0;
        let n = buf.len();
        while pos < n {
            if buf[pos] != SYNC_BYTE {
                // Resync: look for a sync byte followed by another one PACKET_SIZE later.
                let start = pos;
                while pos < n {
                    if buf[pos] == SYNC_BYTE
                        && (pos + PACKET_SIZE >= n || buf[pos + PACKET_SIZE] == SYNC_BYTE)
                    {
                        break;
                    }
                    pos += 1;
                }
                if pos > start {
                    self.events.push(Event::SyncLost { bytes: pos - start });
                }
                if pos >= n {
                    return;
                }
            }
            if pos + PACKET_SIZE > n {
                self.pending.extend_from_slice(&buf[pos..]);
                return;
            }
            let packet: &[u8; PACKET_SIZE] = buf[pos..pos + PACKET_SIZE].try_into().unwrap();
            self.packet(packet);
            pos += PACKET_SIZE;
        }
    }

    fn packet(&mut self, p: &[u8; PACKET_SIZE]) {
        self.packets_seen += 1;
        let transport_error = p[1] & 0x80 != 0;
        let pusi = p[1] & 0x40 != 0;
        let pid = u16::from_be_bytes([p[1] & 0x1F, p[2]]);
        let scrambled = (p[3] >> 6) & 0x03 != 0;
        let afc = (p[3] >> 4) & 0x03;
        let cc = p[3] & 0x0F;

        if pid == PID_NULL || transport_error {
            return;
        }

        let mut payload_start = 4usize;
        let mut random_access = false;
        let mut discontinuity = false;
        if afc & 0x02 != 0 {
            let af_len = p[4] as usize;
            payload_start = 5 + af_len;
            if af_len > 0 {
                let flags = p[5];
                discontinuity = flags & 0x80 != 0;
                random_access = flags & 0x40 != 0;
                let has_pcr = flags & 0x10 != 0;
                if has_pcr && af_len >= 7 {
                    let b = &p[6..12];
                    let base = ((b[0] as u64) << 25)
                        | ((b[1] as u64) << 17)
                        | ((b[2] as u64) << 9)
                        | ((b[3] as u64) << 1)
                        | ((b[4] as u64) >> 7);
                    let ext = (((b[4] & 0x01) as u64) << 8) | b[5] as u64;
                    self.events.push(Event::Pcr {
                        pid,
                        pcr: base * 300 + ext,
                    });
                }
            }
            if payload_start > PACKET_SIZE {
                return; // corrupt adaptation field length
            }
        }
        let has_payload = afc & 0x01 != 0;

        // Continuity check (only packets with payload increment the counter).
        let state = self.pids.entry(pid).or_default();
        if has_payload {
            if let Some(last) = state.last_cc {
                let expected = (last + 1) & 0x0F;
                if cc != expected && cc != last && !discontinuity {
                    self.events.push(Event::Discontinuity { pid });
                    state.pes = None;
                    state.section = None;
                }
            }
            state.last_cc = Some(cc);
        } else {
            return;
        }
        if discontinuity {
            self.events.push(Event::Discontinuity { pid });
        }
        if scrambled || payload_start >= PACKET_SIZE {
            return;
        }
        let payload = &p[payload_start..];

        let is_psi = pid == PID_PAT || self.pat.values().any(|&pmt| pmt == pid);
        if is_psi {
            self.psi_payload(pid, pusi, payload);
        } else if self.es_codecs.contains_key(&pid) {
            self.pes_payload(pid, pusi, random_access, payload);
        }
    }

    fn psi_payload(&mut self, pid: u16, pusi: bool, payload: &[u8]) {
        let state = self.pids.entry(pid).or_default();
        let assembler = state
            .section
            .get_or_insert_with(psi::SectionAssembler::default);
        let sections = assembler.push(pusi, payload);
        for section in sections {
            if !crc::verify(&section) {
                self.events.push(Event::CrcError { pid });
                continue;
            }
            if pid == PID_PAT {
                self.handle_pat(&section);
            } else {
                self.handle_pmt(pid, &section);
            }
        }
    }

    fn handle_pat(&mut self, section: &[u8]) {
        let Some(pat) = psi::parse_pat(section) else {
            return;
        };
        if self.pat_version == Some(pat.version) && !self.pat.is_empty() {
            return;
        }
        self.pat_version = Some(pat.version);
        self.pat.clear();
        for (program, pmt_pid) in pat.programs {
            if program != 0 {
                self.pat.insert(program, pmt_pid);
            }
        }
    }

    fn handle_pmt(&mut self, pmt_pid: u16, section: &[u8]) {
        let Some(pmt) = psi::parse_pmt(section) else {
            return;
        };
        if self.pmt_versions.get(&pmt_pid) == Some(&pmt.version) {
            return;
        }
        self.pmt_versions.insert(pmt_pid, pmt.version);

        // Drop ES state belonging to the previous version of this program.
        if let Some(old) = self.programs.remove(&pmt.program_number) {
            for s in old.streams {
                self.es_codecs.remove(&s.pid);
                self.pids.remove(&s.pid);
                self.seen_keyframe.remove(&s.pid);
            }
        }
        let program = Program {
            number: pmt.program_number,
            pmt_pid,
            pcr_pid: pmt.pcr_pid,
            streams: pmt.streams,
        };
        for s in &program.streams {
            self.es_codecs.insert(s.pid, s.codec);
        }
        self.programs.insert(program.number, program.clone());
        self.events.push(Event::ProgramFound(program));
    }

    fn pes_payload(&mut self, pid: u16, pusi: bool, random_access: bool, payload: &[u8]) {
        if pusi {
            self.flush_pes(pid, true);
        }
        let state = self.pids.entry(pid).or_default();
        match &mut state.pes {
            Some(asm) => asm.push(payload),
            None if pusi => {
                let mut asm = pes::PesAssembler::new(random_access);
                asm.push(payload);
                state.pes = Some(asm);
            }
            None => {} // mid-PES without a start: wait for the next PUSI
        }
        // Bounded PES: emit as soon as we have it all (audio typically).
        if let Some(asm) = &state.pes {
            if asm.is_complete() {
                self.flush_pes(pid, false);
            }
        }
    }

    fn flush_pes(&mut self, pid: u16, _at_pusi: bool) {
        let Some(state) = self.pids.get_mut(&pid) else {
            return;
        };
        let Some(asm) = state.pes.take() else { return };
        let Some(codec) = self.es_codecs.get(&pid).copied() else {
            return;
        };
        let Some(parsed) = asm.finish() else { return };
        if parsed.data.is_empty() {
            return;
        }
        if codec.is_video() {
            let seen = self.seen_keyframe.entry(pid).or_insert(false);
            if !*seen {
                if parsed.random_access || pes::looks_like_keyframe(codec, &parsed.data) {
                    *seen = true;
                } else {
                    return;
                }
            }
        }
        self.events.push(Event::AccessUnit(AccessUnit {
            pid,
            codec,
            pts: parsed.timing.pts,
            dts: parsed.timing.dts,
            random_access: parsed.random_access,
            data: parsed.data,
        }));
    }
}
