//! Packetised elementary stream assembly and header parsing.

use crate::Codec;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TimingInfo {
    pub pts: Option<u64>,
    pub dts: Option<u64>,
}

#[derive(Debug)]
pub struct PesAssembler {
    buf: Vec<u8>,
    random_access: bool,
}

pub struct ParsedPes {
    pub timing: TimingInfo,
    pub random_access: bool,
    pub data: Vec<u8>,
}

impl PesAssembler {
    pub fn new(random_access: bool) -> Self {
        PesAssembler {
            buf: Vec::with_capacity(4096),
            random_access,
        }
    }

    pub fn push(&mut self, payload: &[u8]) {
        self.buf.extend_from_slice(payload);
    }

    /// True when the PES header declares a length and we have all of it.
    pub fn is_complete(&self) -> bool {
        if self.buf.len() < 6 {
            return false;
        }
        let declared = u16::from_be_bytes([self.buf[4], self.buf[5]]) as usize;
        declared != 0 && self.buf.len() >= 6 + declared
    }

    pub fn finish(self) -> Option<ParsedPes> {
        let b = &self.buf;
        if b.len() < 9 || b[0] != 0 || b[1] != 0 || b[2] != 1 {
            return None;
        }
        let stream_id = b[3];
        let declared = u16::from_be_bytes([b[4], b[5]]) as usize;
        let total_end = if declared == 0 {
            b.len()
        } else {
            (6 + declared).min(b.len())
        };

        // Stream ids without the optional header (padding, private_stream_2, ...).
        let no_header = matches!(
            stream_id,
            0xBC | 0xBE | 0xBF | 0xF0 | 0xF1 | 0xF2 | 0xF8 | 0xFF
        );
        if no_header {
            return Some(ParsedPes {
                timing: TimingInfo::default(),
                random_access: self.random_access,
                data: b[6..total_end].to_vec(),
            });
        }
        if b[6] & 0xC0 != 0x80 {
            return None; // not an MPEG-2 PES header
        }
        let pts_dts_flags = (b[7] >> 6) & 0x03;
        let header_len = b[8] as usize;
        let payload_start = 9 + header_len;
        if payload_start > total_end {
            return None;
        }
        let mut timing = TimingInfo::default();
        if pts_dts_flags & 0x02 != 0 && header_len >= 5 {
            timing.pts = read_timestamp(&b[9..14]);
        }
        if pts_dts_flags == 0x03 && header_len >= 10 {
            timing.dts = read_timestamp(&b[14..19]);
        }
        if timing.dts == timing.pts {
            timing.dts = None;
        }
        Some(ParsedPes {
            timing,
            random_access: self.random_access,
            data: b[payload_start..total_end].to_vec(),
        })
    }
}

fn read_timestamp(b: &[u8]) -> Option<u64> {
    if b.len() < 5 {
        return None;
    }
    // Marker bits must be set; tolerate their absence (some muxers get it wrong).
    let ts = (((b[0] >> 1) & 0x07) as u64) << 30
        | (b[1] as u64) << 22
        | (((b[2] >> 1) & 0x7F) as u64) << 15
        | (b[3] as u64) << 7
        | ((b[4] >> 1) & 0x7F) as u64;
    Some(ts)
}

/// Cheap key-frame detection for streams whose muxer does not set the
/// random-access indicator: look for an IDR / IRAP NAL unit.
pub fn looks_like_keyframe(codec: Codec, data: &[u8]) -> bool {
    match codec {
        Codec::H264 => nal_units(data).any(|nal| !nal.is_empty() && nal[0] & 0x1F == 5),
        Codec::H265 => nal_units(data).any(|nal| {
            if nal.is_empty() {
                return false;
            }
            let t = (nal[0] >> 1) & 0x3F;
            (16..=21).contains(&t) // BLA/IDR/CRA
        }),
        Codec::Mpeg2Video => {
            // picture_start_code followed by picture_coding_type == 1 (I-frame)
            data.windows(6).any(|w| {
                w[0] == 0 && w[1] == 0 && w[2] == 1 && w[3] == 0x00 && (w[5] >> 3) & 0x07 == 1
            })
        }
        _ => true,
    }
}

/// Iterate Annex B NAL units (payloads after each start code).
pub fn nal_units(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let n = starts.len();
    (0..n).map(move |k| {
        let s = starts[k];
        let mut e = if k + 1 < n {
            starts[k + 1] - 3
        } else {
            data.len()
        };
        // Trim the zero byte of a 4-byte start code belonging to the next NAL.
        while e > s && data[e - 1] == 0 {
            e -= 1;
        }
        &data[s..e]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_decoding() {
        // PTS = 90000 (1 s): 0b0000_0000_0000_0000_0001_0101_1111_1001_0000
        let pts: u64 = 90_000;
        let b = [
            0x21 | (((pts >> 30) & 0x07) as u8) << 1,
            ((pts >> 22) & 0xFF) as u8,
            0x01 | (((pts >> 15) & 0x7F) as u8) << 1,
            ((pts >> 7) & 0xFF) as u8,
            0x01 | ((pts & 0x7F) as u8) << 1,
        ];
        assert_eq!(read_timestamp(&b), Some(90_000));
    }

    #[test]
    fn nal_iteration_and_idr_detection() {
        let data = [
            0, 0, 0, 1, 0x67, 0xAA, 0, 0, 1, 0x68, 0xBB, 0, 0, 0, 1, 0x65, 0xCC, 0xDD,
        ];
        let nals: Vec<&[u8]> = nal_units(&data).collect();
        assert_eq!(
            nals,
            vec![
                &[0x67, 0xAA][..],
                &[0x68, 0xBB][..],
                &[0x65, 0xCC, 0xDD][..]
            ]
        );
        assert!(looks_like_keyframe(Codec::H264, &data));
        assert!(!looks_like_keyframe(Codec::H264, &[0, 0, 1, 0x41, 0x00]));
        assert!(looks_like_keyframe(Codec::H265, &[0, 0, 1, 0x26, 0x01])); // IDR_W_RADL = 19
        assert!(!looks_like_keyframe(Codec::H265, &[0, 0, 1, 0x02, 0x01])); // TRAIL_R = 1
    }
}
