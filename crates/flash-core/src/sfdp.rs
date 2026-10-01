//! JESD216 Serial Flash Discoverable Parameters (SFDP) parsing.
//!
//! Only the *Basic Flash Parameter Table* is used; that is enough to learn the
//! capacity, address width, page size and erase opcodes of any chip made since ~2012.

use crate::info::{EraseType, FlashInfo, InfoSource};

pub const HEADER_LEN: usize = 8;
pub const PARAM_HEADER_LEN: usize = 8;
/// Number of DWORDs of the basic table we read (JESD216 rev B defines 16).
pub const BASIC_MAX_DWORDS: usize = 16;

/// Location of the basic flash parameter table inside the SFDP address space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BasicTable {
    pub offset: u32,
    pub dwords: usize,
}

/// Validates the SFDP header and returns the number of parameter headers.
pub fn parse_header(raw: &[u8; HEADER_LEN]) -> Option<usize> {
    if &raw[0..4] != b"SFDP" || raw[5] != 1 {
        return None;
    }
    Some(usize::from(raw[6]) + 1)
}

/// Finds the basic table (ID 0xFF00) among the parameter headers.
pub fn find_basic_table(headers: &[u8]) -> Option<BasicTable> {
    headers.chunks_exact(PARAM_HEADER_LEN).find_map(|h| {
        let id = u16::from(h[7]) << 8 | u16::from(h[0]);
        (id == 0xFF00).then(|| BasicTable {
            offset: u32::from_le_bytes([h[4], h[5], h[6], 0]),
            dwords: usize::from(h[3]).clamp(0, BASIC_MAX_DWORDS),
        })
    })
}

fn dword(raw: &[u8], n: usize) -> Option<u32> {
    let b = raw.get(n * 4..n * 4 + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Parses the raw basic table (`dwords * 4` bytes) into [`FlashInfo`].
pub fn parse_basic(jedec: [u8; 3], raw: &[u8]) -> Option<FlashInfo> {
    let dw1 = dword(raw, 0)?;
    let dw2 = dword(raw, 1)?;

    // DWORD 2: density.
    let capacity_bytes: u64 = if dw2 & (1 << 31) == 0 {
        (u64::from(dw2) + 1) / 8
    } else {
        let n = dw2 & 0x7FFF_FFFF;
        if !(3..=40).contains(&n) {
            return None;
        }
        1u64 << (n - 3)
    };
    if capacity_bytes == 0 {
        return None;
    }
    let capacity = u32::try_from(capacity_bytes).unwrap_or(u32::MAX);

    // DWORD 1 bits 18:17: address bytes.
    let addr_bytes = match (dw1 >> 17) & 0b11 {
        0b00 => 3,
        0b01 => {
            if capacity_bytes > 16 * 1024 * 1024 {
                4
            } else {
                3
            }
        }
        0b10 => 4,
        _ => return None,
    };

    // DWORD 8/9: erase types 1..4 (size = 2^N, 0 = unsupported).
    let mut erase_types = [None; 4];
    let mut count = 0;
    for (dw_idx, shift_pairs) in [(7usize, [0u32, 16]), (8, [0, 16])] {
        if let Some(dw) = dword(raw, dw_idx) {
            for s in shift_pairs {
                let size_log2 = (dw >> s) & 0xFF;
                let opcode = ((dw >> (s + 8)) & 0xFF) as u8;
                if size_log2 != 0 && size_log2 < 32 && count < 4 {
                    erase_types[count] = Some(EraseType {
                        opcode,
                        size: 1 << size_log2,
                    });
                    count += 1;
                }
            }
        }
    }
    // Very old (pre rev A) tables only advertise the 4 KiB erase in DWORD 1.
    if count == 0 && dw1 & 0b11 == 0b01 {
        erase_types[0] = Some(EraseType {
            opcode: ((dw1 >> 8) & 0xFF) as u8,
            size: 4096,
        });
    }
    erase_types.sort_unstable_by_key(|e| e.map_or(u32::MAX, |e| e.size));

    // DWORD 11 bits 7:4: page size = 2^N.
    let page_size = dword(raw, 10)
        .map(|dw| 1u16 << ((dw >> 4) & 0xF).min(12))
        .filter(|p| *p >= 16)
        .unwrap_or(256);

    Some(FlashInfo {
        jedec,
        capacity,
        addr_bytes,
        page_size,
        erase_types,
        source: InfoSource::Sfdp,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Basic table of a Winbond W25Q32JV (JESD216 rev B subset).
    pub fn w25q32_table() -> [u8; 64] {
        let mut t = [0u8; 64];
        let dws: [u32; 16] = [
            0xFF20_E5FD, // DWORD1: 4K erase opcode 0x20, 3-byte addr
            0x01FF_FFFF, // DWORD2: (0x1FFFFFF + 1) bits = 32 Mbit
            0x6B08_3B04, // DWORD3
            0xBB44_EB08, // DWORD4
            0xFFFF_FFFF, // DWORD5
            0xFFFF_BB04, // DWORD6
            0xFF00_FFFF, // DWORD7
            0x520F_200C, // DWORD8: type1 4K op 0x20, type2 32K op 0x52
            0xFF00_D810, // DWORD9: type3 64K op 0xD8, type4 unused
            0,           // DWORD10
            8 << 4,      // DWORD11: page size 2^8
            0,
            0,
            0,
            0,
            0,
        ];
        for (i, d) in dws.iter().enumerate() {
            t[i * 4..i * 4 + 4].copy_from_slice(&d.to_le_bytes());
        }
        t
    }

    #[test]
    fn parses_w25q32() {
        let info = parse_basic([0xEF, 0x40, 0x16], &w25q32_table()).unwrap();
        assert_eq!(info.capacity, 4 * 1024 * 1024);
        assert_eq!(info.addr_bytes, 3);
        assert_eq!(info.page_size, 256);
        let e: Vec<_> = info.erase_types().collect();
        assert_eq!(
            e,
            vec![
                EraseType {
                    opcode: 0x20,
                    size: 4096
                },
                EraseType {
                    opcode: 0x52,
                    size: 32768
                },
                EraseType {
                    opcode: 0xD8,
                    size: 65536
                },
            ]
        );
    }

    #[test]
    fn header_and_find() {
        assert_eq!(parse_header(b"SFDP\x06\x01\x00\xFF"), Some(1));
        assert_eq!(parse_header(b"NOPE\x06\x01\x00\xFF"), None);
        // id 0x00, rev 6.1, 16 dwords, ptr 0x30, id msb 0xFF
        let h = [0x00, 0x06, 0x01, 0x10, 0x30, 0x00, 0x00, 0xFF];
        assert_eq!(
            find_basic_table(&h),
            Some(BasicTable {
                offset: 0x30,
                dwords: 16
            })
        );
    }

    #[test]
    fn density_exponent_form() {
        let mut t = w25q32_table();
        // 1 Gbit = 2^30 bits -> bit31 set, N = 30
        t[4..8].copy_from_slice(&(0x8000_0000u32 | 30).to_le_bytes());
        let info = parse_basic([0, 0, 0], &t).unwrap();
        assert_eq!(info.capacity, 128 * 1024 * 1024);
    }

    #[test]
    fn four_byte_flag() {
        let mut t = w25q32_table();
        t[0..4].copy_from_slice(&(0xFF20_E5FDu32 | (0b10 << 17)).to_le_bytes());
        assert_eq!(parse_basic([0; 3], &t).unwrap().addr_bytes, 4);
    }
}
