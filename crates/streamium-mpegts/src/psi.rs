//! Program specific information: section reassembly, PAT and PMT parsing.

use crate::{Codec, Stream};

/// Reassembles PSI sections that may span several TS packets, and splits
/// packets that carry several short sections.
#[derive(Debug, Default)]
pub struct SectionAssembler {
    buf: Vec<u8>,
    expected: Option<usize>,
}

impl SectionAssembler {
    /// Returns every complete section finished by this payload.
    pub fn push(&mut self, pusi: bool, payload: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        let mut data = payload;
        if pusi {
            if data.is_empty() {
                return out;
            }
            let pointer = data[0] as usize;
            // Bytes before the pointer target complete the previous section.
            if pointer > 0 && self.expected.is_some() {
                let tail_end = (1 + pointer).min(data.len());
                self.buf.extend_from_slice(&data[1..tail_end]);
                self.try_finish(&mut out);
            }
            self.buf.clear();
            self.expected = None;
            if 1 + pointer >= data.len() {
                return out;
            }
            data = &data[1 + pointer..];
            // Several sections may follow each other; 0xFF is stuffing.
            while !data.is_empty() && data[0] != 0xFF {
                if data.len() < 3 {
                    self.buf.extend_from_slice(data);
                    break;
                }
                let len = 3 + (u16::from_be_bytes([data[1] & 0x0F, data[2]]) as usize);
                if len > data.len() {
                    self.buf.extend_from_slice(data);
                    self.expected = Some(len);
                    break;
                }
                out.push(data[..len].to_vec());
                data = &data[len..];
            }
        } else if self.expected.is_some() || !self.buf.is_empty() {
            self.buf.extend_from_slice(data);
            if self.expected.is_none() && self.buf.len() >= 3 {
                self.expected =
                    Some(3 + (u16::from_be_bytes([self.buf[1] & 0x0F, self.buf[2]]) as usize));
            }
            self.try_finish(&mut out);
        }
        out
    }

    fn try_finish(&mut self, out: &mut Vec<Vec<u8>>) {
        if let Some(len) = self.expected {
            if self.buf.len() >= len {
                out.push(self.buf[..len].to_vec());
                self.buf.clear();
                self.expected = None;
            }
        }
    }
}

pub struct Pat {
    pub version: u8,
    pub programs: Vec<(u16, u16)>,
}

pub fn parse_pat(section: &[u8]) -> Option<Pat> {
    if section.len() < 12 || section[0] != 0x00 {
        return None;
    }
    let section_length = u16::from_be_bytes([section[1] & 0x0F, section[2]]) as usize;
    let end = (3 + section_length).min(section.len()) - 4; // exclude CRC
    let version = (section[5] >> 1) & 0x1F;
    let current_next = section[5] & 0x01 != 0;
    if !current_next {
        return None;
    }
    let mut programs = Vec::new();
    let mut i = 8;
    while i + 4 <= end {
        let program = u16::from_be_bytes([section[i], section[i + 1]]);
        let pid = u16::from_be_bytes([section[i + 2] & 0x1F, section[i + 3]]);
        programs.push((program, pid));
        i += 4;
    }
    Some(Pat { version, programs })
}

pub struct Pmt {
    pub version: u8,
    pub program_number: u16,
    pub pcr_pid: u16,
    pub streams: Vec<Stream>,
}

pub fn parse_pmt(section: &[u8]) -> Option<Pmt> {
    if section.len() < 16 || section[0] != 0x02 {
        return None;
    }
    let section_length = u16::from_be_bytes([section[1] & 0x0F, section[2]]) as usize;
    let end = (3 + section_length).min(section.len()).checked_sub(4)?;
    let program_number = u16::from_be_bytes([section[3], section[4]]);
    let version = (section[5] >> 1) & 0x1F;
    if section[5] & 0x01 == 0 {
        return None;
    }
    let pcr_pid = u16::from_be_bytes([section[8] & 0x1F, section[9]]);
    let program_info_length = u16::from_be_bytes([section[10] & 0x0F, section[11]]) as usize;
    let mut i = 12 + program_info_length;
    let mut streams = Vec::new();
    while i + 5 <= end {
        let stream_type = section[i];
        let pid = u16::from_be_bytes([section[i + 1] & 0x1F, section[i + 2]]);
        let es_info_length = u16::from_be_bytes([section[i + 3] & 0x0F, section[i + 4]]) as usize;
        let desc_start = i + 5;
        let desc_end = (desc_start + es_info_length).min(end);
        let descriptors = &section[desc_start..desc_end];
        if let Some(codec) = codec_for(stream_type, descriptors) {
            streams.push(Stream {
                pid,
                codec,
                stream_type,
                language: language_of(descriptors),
            });
        }
        i = desc_end;
    }
    Some(Pmt {
        version,
        program_number,
        pcr_pid,
        streams,
    })
}

fn descriptors(bytes: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut i = 0;
    std::iter::from_fn(move || {
        if i + 2 > bytes.len() {
            return None;
        }
        let tag = bytes[i];
        let len = bytes[i + 1] as usize;
        let start = i + 2;
        let end = (start + len).min(bytes.len());
        i = end;
        Some((tag, &bytes[start..end]))
    })
}

fn codec_for(stream_type: u8, descs: &[u8]) -> Option<Codec> {
    Some(match stream_type {
        0x01 | 0x02 => Codec::Mpeg2Video,
        0x03 | 0x04 => Codec::MpegAudio,
        0x0F => Codec::AacAdts,
        0x11 => Codec::AacLatm,
        0x1B => Codec::H264,
        0x24 => Codec::H265,
        0x81 => Codec::Ac3,
        0x87 => Codec::Eac3,
        0x8A | 0x82 | 0x85 | 0x86 => Codec::Dts,
        0x06 => {
            // Private PES data: identify by descriptor tag.
            let mut found = None;
            for (tag, body) in descriptors(descs) {
                found = match tag {
                    0x6A => Some(Codec::Ac3),
                    0x7A => Some(Codec::Eac3),
                    0x7B => Some(Codec::Dts),
                    0x59 => Some(Codec::DvbSubtitle),
                    0x56 => Some(Codec::Teletext),
                    0x05 if body.len() >= 4 => match &body[..4] {
                        b"AC-3" => Some(Codec::Ac3),
                        b"EAC3" => Some(Codec::Eac3),
                        b"DTS1" | b"DTS2" | b"DTS3" => Some(Codec::Dts),
                        b"HEVC" => Some(Codec::H265),
                        _ => None,
                    },
                    _ => None,
                };
                if found.is_some() {
                    break;
                }
            }
            found?
        }
        0x05 | 0x0D | 0x80 | 0x90..=0x9F | 0xC0..=0xFF => return None, // tables, data, DRM
        other => Codec::Other(other),
    })
}

fn language_of(descs: &[u8]) -> Option<String> {
    descriptors(descs)
        .find(|(tag, body)| *tag == 0x0A && body.len() >= 3)
        .and_then(|(_, body)| {
            let code = &body[..3];
            code.iter()
                .all(|c| c.is_ascii_alphabetic())
                .then(|| String::from_utf8_lossy(code).to_lowercase())
        })
}
