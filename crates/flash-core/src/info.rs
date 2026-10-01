//! Description of a detected flash chip.

/// One block/sector erase granularity supported by the chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EraseType {
    pub opcode: u8,
    /// Erase size in bytes (always a power of two).
    pub size: u32,
}

/// Where the information in [`FlashInfo`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoSource {
    /// Parsed from the chip's JESD216 SFDP tables.
    Sfdp,
    /// Chip has no SFDP; JEDEC-standard defaults + capacity guessed from the JEDEC ID.
    Default,
    /// Set explicitly by the user.
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FlashInfo {
    /// Manufacturer, memory type, capacity (response of 0x9F).
    pub jedec: [u8; 3],
    /// Capacity in bytes.
    pub capacity: u32,
    /// 3 or 4 byte addressing.
    pub addr_bytes: u8,
    /// Program page size in bytes.
    pub page_size: u16,
    /// Erase types sorted by ascending size. Unused slots are `None`.
    pub erase_types: [Option<EraseType>; 4],
    pub source: InfoSource,
}

/// JEDEC-standard erase opcodes shared by practically every 25-series chip.
pub const DEFAULT_ERASE_TYPES: [Option<EraseType>; 4] = [
    Some(EraseType {
        opcode: 0x20,
        size: 4 * 1024,
    }),
    Some(EraseType {
        opcode: 0x52,
        size: 32 * 1024,
    }),
    Some(EraseType {
        opcode: 0xD8,
        size: 64 * 1024,
    }),
    None,
];

impl FlashInfo {
    /// Info for a chip without SFDP, using JEDEC-standard defaults.
    pub fn from_jedec_defaults(jedec: [u8; 3]) -> Self {
        let capacity = capacity_from_jedec(jedec).unwrap_or(0);
        Self {
            jedec,
            capacity,
            addr_bytes: if capacity > 16 * 1024 * 1024 { 4 } else { 3 },
            page_size: 256,
            erase_types: DEFAULT_ERASE_TYPES,
            source: InfoSource::Default,
        }
    }

    /// Info for a chip where the user provided capacity and address width.
    pub fn manual(jedec: [u8; 3], capacity: u32, addr_bytes: u8) -> Self {
        Self {
            jedec,
            capacity,
            addr_bytes,
            page_size: 256,
            erase_types: DEFAULT_ERASE_TYPES,
            source: InfoSource::Manual,
        }
    }

    pub fn erase_types(&self) -> impl Iterator<Item = EraseType> + '_ {
        self.erase_types.iter().flatten().copied()
    }

    /// The smallest erase granularity (the alignment required for erasing).
    pub fn min_erase_size(&self) -> Option<u32> {
        self.erase_types().map(|e| e.size).min()
    }
}

/// Best-effort capacity guess from the third JEDEC ID byte.
///
/// Most vendors encode the capacity as `log2(bytes)` in that byte (0x14 = 1 MiB,
/// 0x17 = 8 MiB, 0x18 = 16 MiB ...). Micron uses 0x20 for 64 MiB.
pub fn capacity_from_jedec(jedec: [u8; 3]) -> Option<u32> {
    let shift = match jedec[2] {
        n @ 0x10..=0x1F => u32::from(n),
        n @ 0x20..=0x22 => u32::from(n - 0x20) + 26,
        _ => return None,
    };
    1u32.checked_shl(shift)
}

/// Vendor name for the first JEDEC ID byte (bank 1 only).
pub fn manufacturer_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0x01 => "Spansion/Cypress",
        0x1C => "EON",
        0x1F => "Adesto/Atmel",
        0x20 => "Micron/ST",
        0x37 => "AMIC",
        0x5E => "Zbit",
        0x68 => "Boya",
        0x85 => "Puya",
        0x8C => "ESMT",
        0x9D => "ISSI",
        0xA1 => "Fudan",
        0xBF => "SST/Microchip",
        0xC2 => "Macronix",
        0xC8 => "GigaDevice",
        0xEF => "Winbond",
        0x0B => "XTX",
        _ => return None,
    })
}
