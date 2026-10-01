use super::*;
use crate::info::InfoSource;
use core::convert::Infallible;
use embassy_futures::block_on;
use embedded_hal_async::spi::ErrorType;

struct NoDelay;
impl DelayNs for NoDelay {
    async fn delay_ns(&mut self, _: u32) {}
}

/// Behavioural model of a NOR flash chip (AND-programming, erase to 0xFF, WEL, WIP, 4B mode).
struct Sim {
    mem: Vec<u8>,
    jedec: [u8; 3],
    sfdp: Option<Vec<u8>>,
    wel: bool,
    four_byte: bool,
    busy_polls: u32,
    busy_left: u32,
    /// Opcodes seen, for assertions.
    log: Vec<u8>,
    addr_bytes_capable: bool,
}

impl Sim {
    fn new(size: usize, jedec: [u8; 3], sfdp: Option<Vec<u8>>) -> Self {
        Self {
            mem: vec![0xFF; size],
            jedec,
            sfdp,
            wel: false,
            four_byte: false,
            busy_polls: 2,
            busy_left: 0,
            log: vec![],
            addr_bytes_capable: size > 16 << 20,
        }
    }
    fn addr_len(&self) -> usize {
        if self.four_byte { 4 } else { 3 }
    }
    fn parse_addr(&self, b: &[u8]) -> usize {
        b[..self.addr_len()]
            .iter()
            .fold(0usize, |a, &x| a << 8 | x as usize)
    }
}

impl ErrorType for Sim {
    type Error = Infallible;
}

impl SpiDevice for Sim {
    async fn transaction(&mut self, ops: &mut [Operation<'_, u8>]) -> Result<(), Infallible> {
        // Collect the written bytes, then serve reads.
        let mut written: Vec<u8> = vec![];
        for o in ops.iter() {
            if let Operation::Write(w) = o {
                written.extend_from_slice(w);
            }
        }
        let opc = written[0];
        self.log.push(opc);
        let args = &written[1..];
        let mut read_data: Vec<u8> = vec![];
        match opc {
            0x9F => read_data = self.jedec.to_vec(),
            0x05 => {
                let sr = if self.busy_left > 0 {
                    self.busy_left -= 1;
                    1
                } else {
                    0
                };
                read_data = vec![sr | if self.wel { 2 } else { 0 }];
            }
            0x06 => self.wel = true,
            0xB7 => {
                assert!(self.addr_bytes_capable, "chip has no 4B mode");
                self.four_byte = true
            }
            0xE9 => self.four_byte = false,
            0xAB => {}
            0x5A => {
                let a = (args[0] as usize) << 16 | (args[1] as usize) << 8 | args[2] as usize;
                // args[3] is the dummy byte
                let want: usize = ops
                    .iter()
                    .map(|o| {
                        if let Operation::Read(r) = o {
                            r.len()
                        } else {
                            0
                        }
                    })
                    .sum();
                read_data = match &self.sfdp {
                    Some(s) => (0..want).map(|i| *s.get(a + i).unwrap_or(&0xFF)).collect(),
                    None => vec![0xFF; want],
                };
            }
            0x03 => {
                let a = self.parse_addr(args);
                let want: usize = ops
                    .iter()
                    .map(|o| {
                        if let Operation::Read(r) = o {
                            r.len()
                        } else {
                            0
                        }
                    })
                    .sum();
                read_data = self.mem[a..a + want].to_vec();
            }
            0x02 => {
                assert!(self.wel, "program without WREN");
                let a = self.parse_addr(args);
                let data = &args[self.addr_len()..];
                let page = a / 256;
                assert!(
                    (a + data.len() - 1) / 256 == page,
                    "program crosses page boundary"
                );
                for (i, d) in data.iter().enumerate() {
                    self.mem[a + i] &= d;
                }
                self.wel = false;
                self.busy_left = self.busy_polls;
            }
            0x20 | 0x52 | 0xD8 => {
                assert!(self.wel, "erase without WREN");
                let a = self.parse_addr(args);
                let size = match opc {
                    0x20 => 4096,
                    0x52 => 32768,
                    _ => 65536,
                };
                assert_eq!(a % size, 0, "unaligned erase");
                self.mem[a..a + size].fill(0xFF);
                self.wel = false;
                self.busy_left = self.busy_polls;
            }
            0x01 => {
                assert!(self.wel);
                self.wel = false;
            }
            other => panic!("unsupported opcode {other:#x}"),
        }
        let mut it = read_data.into_iter();
        for o in ops.iter_mut() {
            if let Operation::Read(r) = o {
                for b in r.iter_mut() {
                    *b = it.next().unwrap_or(0xFF);
                }
            }
        }
        Ok(())
    }
}

fn w25q32_sfdp() -> Vec<u8> {
    let mut s = vec![0xFFu8; 256];
    s[0..8].copy_from_slice(b"SFDP\x06\x01\x00\xFF");
    // param header 0: id 0x00, rev 6.1, 16 dwords at 0x30, id msb 0xFF
    s[8..16].copy_from_slice(&[0x00, 0x06, 0x01, 0x10, 0x30, 0x00, 0x00, 0xFF]);
    let dws: [u32; 16] = [
        0xFF20_E5FD,
        0x01FF_FFFF,
        0x6B08_3B04,
        0xBB44_EB08,
        0xFFFF_FFFF,
        0xFFFF_BB04,
        0xFF00_FFFF,
        0x520F_200C,
        0xFF00_D810,
        0,
        8 << 4,
        0,
        0,
        0,
        0,
        0,
    ];
    for (i, d) in dws.iter().enumerate() {
        s[0x30 + i * 4..0x30 + i * 4 + 4].copy_from_slice(&d.to_le_bytes());
    }
    s
}

#[test]
fn probe_via_sfdp() {
    let mut f = Flash::new(
        Sim::new(4 << 20, [0xEF, 0x40, 0x16], Some(w25q32_sfdp())),
        NoDelay,
    );
    let info = block_on(f.probe()).unwrap();
    assert_eq!(info.source, InfoSource::Sfdp);
    assert_eq!(info.capacity, 4 << 20);
    assert_eq!(info.erase_types().count(), 3);
}

#[test]
fn probe_without_sfdp_uses_defaults() {
    // e.g. a legacy 25-series part: ID says 2 MiB, no SFDP.
    let mut f = Flash::new(Sim::new(2 << 20, [0xC2, 0x20, 0x15], None), NoDelay);
    let info = block_on(f.probe()).unwrap();
    assert_eq!(info.source, InfoSource::Default);
    assert_eq!(info.capacity, 2 << 20);
    assert_eq!(info.addr_bytes, 3);
}

#[test]
fn no_device() {
    let mut f = Flash::new(Sim::new(1024, [0xFF; 3], None), NoDelay);
    assert!(matches!(block_on(f.probe()), Err(Error::NoDevice)));
    let mut f = Flash::new(Sim::new(1024, [0; 3], None), NoDelay);
    assert!(matches!(block_on(f.probe()), Err(Error::NoDevice)));
}

#[test]
fn erase_program_read_roundtrip() {
    let mut f = Flash::new(
        Sim::new(4 << 20, [0xEF, 0x40, 0x16], Some(w25q32_sfdp())),
        NoDelay,
    );
    block_on(f.probe()).unwrap();
    // pre-dirty some area
    f.dev.mem[0x1000..0x3000].fill(0x00);
    block_on(f.erase(0x1000, 0x2000)).unwrap();
    assert!(f.dev.mem[0x1000..0x3000].iter().all(|&b| b == 0xFF));

    let data: Vec<u8> = (0..1000u32).map(|i| (i * 7) as u8).collect();
    block_on(f.program(0x10F0, &data)).unwrap(); // crosses several pages, unaligned start
    let mut back = vec![0u8; data.len()];
    block_on(f.read(0x10F0, &mut back)).unwrap();
    assert_eq!(back, data);
}

#[test]
fn full_chip_erase_uses_big_blocks() {
    let mut f = Flash::new(
        Sim::new(4 << 20, [0xEF, 0x40, 0x16], Some(w25q32_sfdp())),
        NoDelay,
    );
    block_on(f.probe()).unwrap();
    f.dev.log.clear();
    block_on(f.erase(0, 4 << 20)).unwrap();
    assert_eq!(f.dev.log.iter().filter(|&&o| o == 0xD8).count(), 64);
    assert_eq!(f.dev.log.iter().filter(|&&o| o == 0x20).count(), 0);
}

#[test]
fn four_byte_addressing_is_scoped() {
    // 32 MiB part; manual info with 4-byte addressing.
    let mut f = Flash::new(Sim::new(32 << 20, [0xEF, 0x40, 0x19], None), NoDelay);
    f.set_info(FlashInfo::manual([0xEF, 0x40, 0x19], 32 << 20, 4));
    block_on(f.erase(20 << 20, 4096)).unwrap();
    block_on(f.program(20 << 20, &[1, 2, 3])).unwrap();
    let mut b = [0u8; 3];
    block_on(f.read(20 << 20, &mut b)).unwrap();
    assert_eq!(b, [1, 2, 3]);
    assert!(!f.dev.four_byte, "chip must be returned to 3-byte mode");
}

#[test]
fn range_and_alignment_errors() {
    let mut f = Flash::new(Sim::new(2 << 20, [0xC2, 0x20, 0x15], None), NoDelay);
    assert!(matches!(
        block_on(f.read(0, &mut [0u8; 1])),
        Err(Error::NotProbed)
    ));
    block_on(f.probe()).unwrap();
    assert!(matches!(
        block_on(f.read((2 << 20) - 1, &mut [0u8; 2])),
        Err(Error::OutOfRange)
    ));
    assert!(matches!(
        block_on(f.erase(1, 4096)),
        Err(Error::Plan(PlanError::Unaligned { .. }))
    ));
}

#[test]
fn timeout_when_chip_stays_busy() {
    let mut f = Flash::new(Sim::new(2 << 20, [0xC2, 0x20, 0x15], None), NoDelay);
    block_on(f.probe()).unwrap();
    f.dev.busy_polls = u32::MAX;
    assert!(matches!(block_on(f.program(0, &[0])), Err(Error::Timeout)));
}
