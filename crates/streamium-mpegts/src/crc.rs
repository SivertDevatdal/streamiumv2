//! CRC-32/MPEG-2: polynomial 0x04C11DB7, init 0xFFFFFFFF, no reflection, no final XOR.

const fn make_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = (i as u32) << 24;
        let mut j = 0;
        while j < 8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
            j += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
}

static TABLE: [u32; 256] = make_table();

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = (crc << 8) ^ TABLE[((crc >> 24) as u8 ^ b) as usize];
    }
    crc
}

/// A PSI section is valid when the CRC over the whole section (including the
/// trailing CRC bytes) equals zero.
pub fn verify(section: &[u8]) -> bool {
    section.len() >= 4 && crc32(section) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vector() {
        // "123456789" under CRC-32/MPEG-2 is 0x0376E6E7.
        assert_eq!(crc32(b"123456789"), 0x0376_E6E7);
    }

    #[test]
    fn appended_crc_verifies() {
        let mut s = b"\x00\xb0\x0d\x00\x01\xc1\x00\x00\x00\x01\xf0\x00".to_vec();
        let c = crc32(&s);
        s.extend_from_slice(&c.to_be_bytes());
        assert!(verify(&s));
        s[3] ^= 0x01;
        assert!(!verify(&s));
    }
}
