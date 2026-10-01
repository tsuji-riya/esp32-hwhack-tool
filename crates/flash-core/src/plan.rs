//! Pure planning helpers: erase decomposition and page splitting.

use crate::info::{EraseType, FlashInfo};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EraseOp {
    pub opcode: u8,
    pub addr: u32,
    pub size: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// No erase types known for this chip.
    NoEraseTypes,
    /// `addr` or `len` is not a multiple of the smallest erase size.
    Unaligned {
        required: u32,
    },
    OutOfRange,
}

/// Iterator over the erase operations covering `[addr, addr + len)`.
///
/// At every step the largest erase type that is both aligned at the current address
/// and fits the remaining length is chosen.
#[derive(Clone, Debug)]
pub struct ErasePlan {
    types: [Option<EraseType>; 4],
    addr: u32,
    remaining: u32,
}

impl ErasePlan {
    pub fn new(info: &FlashInfo, addr: u32, len: u32) -> Result<Self, PlanError> {
        let min = info.min_erase_size().ok_or(PlanError::NoEraseTypes)?;
        if !addr.is_multiple_of(min) || !len.is_multiple_of(min) {
            return Err(PlanError::Unaligned { required: min });
        }
        match addr.checked_add(len) {
            Some(end) if end <= info.capacity => {}
            _ => return Err(PlanError::OutOfRange),
        }
        Ok(Self {
            types: info.erase_types,
            addr,
            remaining: len,
        })
    }
}

impl Iterator for ErasePlan {
    type Item = EraseOp;

    fn next(&mut self) -> Option<EraseOp> {
        if self.remaining == 0 {
            return None;
        }
        let t = self
            .types
            .iter()
            .flatten()
            .filter(|t| t.size <= self.remaining && self.addr.is_multiple_of(t.size))
            .max_by_key(|t| t.size)?;
        let op = EraseOp {
            opcode: t.opcode,
            addr: self.addr,
            size: t.size,
        };
        self.addr += t.size;
        self.remaining -= t.size;
        Some(op)
    }
}

/// Splits `data_len` bytes starting at `addr` into chunks that never cross a page boundary.
/// Yields `(address, offset_in_data, length)`.
pub fn page_chunks(
    addr: u32,
    data_len: usize,
    page_size: u16,
) -> impl Iterator<Item = (u32, usize, usize)> {
    let page = usize::from(page_size.max(1));
    let mut off = 0usize;
    core::iter::from_fn(move || {
        if off >= data_len {
            return None;
        }
        let a = addr as usize + off;
        let n = (page - a % page).min(data_len - off);
        let item = (a as u32, off, n);
        off += n;
        Some(item)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::info::{FlashInfo, InfoSource};

    fn info() -> FlashInfo {
        FlashInfo {
            jedec: [0; 3],
            capacity: 4 << 20,
            addr_bytes: 3,
            page_size: 256,
            erase_types: crate::info::DEFAULT_ERASE_TYPES,
            source: InfoSource::Default,
        }
    }

    #[test]
    fn greedy_erase() {
        // 4K..: start at 60K, length 76K -> 4K(60K) then 64K(64K) then 4K(128K)x2 ... verify coverage
        let ops: Vec<_> = ErasePlan::new(&info(), 60 * 1024, 76 * 1024)
            .unwrap()
            .collect();
        assert_eq!(
            ops[0],
            EraseOp {
                opcode: 0x20,
                addr: 60 * 1024,
                size: 4096
            }
        );
        assert_eq!(
            ops[1],
            EraseOp {
                opcode: 0xD8,
                addr: 64 * 1024,
                size: 65536
            }
        );
        assert_eq!(ops.iter().map(|o| o.size).sum::<u32>(), 76 * 1024);
        let mut next = 60 * 1024;
        for o in ops {
            assert_eq!(o.addr, next);
            next += o.size;
        }
    }

    #[test]
    fn rejects_bad_ranges() {
        assert_eq!(
            ErasePlan::new(&info(), 100, 4096).unwrap_err(),
            PlanError::Unaligned { required: 4096 }
        );
        assert_eq!(
            ErasePlan::new(&info(), 0, 8 << 20).unwrap_err(),
            PlanError::OutOfRange
        );
    }

    #[test]
    fn pages() {
        let v: Vec<_> = page_chunks(250, 20, 256).collect();
        assert_eq!(v, vec![(250, 0, 6), (256, 6, 14)]);
        assert_eq!(page_chunks(0, 0, 256).count(), 0);
        assert_eq!(page_chunks(0, 512, 256).count(), 2);
    }
}
