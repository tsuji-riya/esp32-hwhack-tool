//! Async flash driver on top of `embedded_hal_async::spi::SpiDevice`.

use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::spi::{Operation, SpiDevice};

use crate::info::{FlashInfo, capacity_from_jedec};
use crate::plan::{EraseOp, ErasePlan, PlanError, page_chunks};
use crate::sfdp;

mod op {
    pub const WRSR1: u8 = 0x01;
    pub const PAGE_PROGRAM: u8 = 0x02;
    pub const READ: u8 = 0x03;
    pub const RDSR1: u8 = 0x05;
    pub const WREN: u8 = 0x06;
    pub const READ_SFDP: u8 = 0x5A;
    pub const RELEASE_POWER_DOWN: u8 = 0xAB;
    pub const ENTER_4B: u8 = 0xB7;
    pub const EXIT_4B: u8 = 0xE9;
    pub const RDID: u8 = 0x9F;
}

const SR_WIP: u8 = 1 << 0;
const PROGRAM_TIMEOUT_MS: u32 = 100;
const ERASE_TIMEOUT_MS: u32 = 5_000;

#[derive(Debug)]
pub enum Error<E> {
    Spi(E),
    /// JEDEC ID read back as all-zero / all-one: nothing is answering.
    NoDevice,
    /// An operation needing [`FlashInfo`] was called before `probe`/`set_info`.
    NotProbed,
    Plan(PlanError),
    OutOfRange,
    /// Write-in-progress did not clear in time.
    Timeout,
}

impl<E> From<PlanError> for Error<E> {
    fn from(e: PlanError) -> Self {
        Error::Plan(e)
    }
}

pub struct Flash<D, T> {
    dev: D,
    delay: T,
    info: Option<FlashInfo>,
}

impl<D: SpiDevice, T: DelayNs> Flash<D, T> {
    pub fn new(dev: D, delay: T) -> Self {
        Self {
            dev,
            delay,
            info: None,
        }
    }

    pub fn info(&self) -> Option<&FlashInfo> {
        self.info.as_ref()
    }

    /// Overrides detected parameters (e.g. for a chip with a wrong/missing SFDP).
    pub fn set_info(&mut self, info: FlashInfo) {
        self.info = Some(info);
    }

    pub fn release(self) -> D {
        self.dev
    }

    fn info_or_err(&self) -> Result<FlashInfo, Error<D::Error>> {
        self.info.ok_or(Error::NotProbed)
    }

    async fn cmd(&mut self, opcode: u8) -> Result<(), Error<D::Error>> {
        self.dev.write(&[opcode]).await.map_err(Error::Spi)
    }

    /// Encodes `opcode` + address; returns the used prefix length.
    fn header(opcode: u8, addr: u32, addr_bytes: u8, buf: &mut [u8; 5]) -> usize {
        buf[0] = opcode;
        let a = addr.to_be_bytes();
        if addr_bytes == 4 {
            buf[1..5].copy_from_slice(&a);
            5
        } else {
            buf[1..4].copy_from_slice(&a[1..]);
            4
        }
    }

    pub async fn read_jedec_id(&mut self) -> Result<[u8; 3], Error<D::Error>> {
        let mut id = [0u8; 3];
        self.dev
            .transaction(&mut [Operation::Write(&[op::RDID]), Operation::Read(&mut id)])
            .await
            .map_err(Error::Spi)?;
        Ok(id)
    }

    /// Reads from the SFDP address space (always 3 address bytes + 1 dummy byte).
    pub async fn read_sfdp(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Error<D::Error>> {
        let a = addr.to_be_bytes();
        let hdr = [op::READ_SFDP, a[1], a[2], a[3], 0x00];
        self.dev
            .transaction(&mut [Operation::Write(&hdr), Operation::Read(buf)])
            .await
            .map_err(Error::Spi)
    }

    pub async fn read_status(&mut self) -> Result<u8, Error<D::Error>> {
        let mut sr = [0u8; 1];
        self.dev
            .transaction(&mut [Operation::Write(&[op::RDSR1]), Operation::Read(&mut sr)])
            .await
            .map_err(Error::Spi)?;
        Ok(sr[0])
    }

    /// Detects the chip: JEDEC ID, then SFDP, falling back to JEDEC defaults.
    pub async fn probe(&mut self) -> Result<FlashInfo, Error<D::Error>> {
        // Wake the chip if it was left in deep power-down (ignored otherwise).
        self.cmd(op::RELEASE_POWER_DOWN).await?;
        self.delay.delay_us(50).await;

        let jedec = self.read_jedec_id().await?;
        if jedec == [0; 3] || jedec == [0xFF; 3] {
            return Err(Error::NoDevice);
        }

        let info = match self.probe_sfdp(jedec).await? {
            Some(info) => info,
            None => FlashInfo::from_jedec_defaults(jedec),
        };
        self.info = Some(info);
        Ok(info)
    }

    async fn probe_sfdp(&mut self, jedec: [u8; 3]) -> Result<Option<FlashInfo>, Error<D::Error>> {
        let mut header = [0u8; sfdp::HEADER_LEN];
        self.read_sfdp(0, &mut header).await?;
        let Some(nph) = sfdp::parse_header(&header) else {
            return Ok(None);
        };

        let mut headers = [0u8; 8 * sfdp::PARAM_HEADER_LEN];
        let n = nph.min(8) * sfdp::PARAM_HEADER_LEN;
        self.read_sfdp(sfdp::HEADER_LEN as u32, &mut headers[..n])
            .await?;
        let Some(table) = sfdp::find_basic_table(&headers[..n]) else {
            return Ok(None);
        };

        let mut raw = [0u8; sfdp::BASIC_MAX_DWORDS * 4];
        let len = table.dwords * 4;
        self.read_sfdp(table.offset, &mut raw[..len]).await?;

        let mut info = sfdp::parse_basic(jedec, &raw[..len]);
        // Some chips report garbage; sanity check against the JEDEC ID guess.
        if let (Some(i), Some(guess)) = (&mut info, capacity_from_jedec(jedec))
            && i.capacity != guess
            && i.capacity < 1024
        {
            i.capacity = guess;
        }
        Ok(info)
    }

    async fn write_enable(&mut self) -> Result<(), Error<D::Error>> {
        self.cmd(op::WREN).await
    }

    async fn wait_ready(&mut self, poll_us: u32, timeout_ms: u32) -> Result<(), Error<D::Error>> {
        let mut waited_us: u64 = 0;
        loop {
            if self.read_status().await? & SR_WIP == 0 {
                return Ok(());
            }
            if waited_us > u64::from(timeout_ms) * 1000 {
                return Err(Error::Timeout);
            }
            self.delay.delay_us(poll_us).await;
            waited_us += u64::from(poll_us);
        }
    }

    /// Clears the block-protect bits (status register 1 := 0).
    pub async fn clear_protection(&mut self) -> Result<(), Error<D::Error>> {
        self.write_enable().await?;
        self.dev
            .write(&[op::WRSR1, 0x00])
            .await
            .map_err(Error::Spi)?;
        self.wait_ready(1_000, ERASE_TIMEOUT_MS).await
    }

    async fn enter_4b(&mut self, info: &FlashInfo) -> Result<(), Error<D::Error>> {
        if info.addr_bytes == 4 {
            self.write_enable().await?; // required by some vendors, ignored by others
            self.cmd(op::ENTER_4B).await?;
        }
        Ok(())
    }

    /// Leave the chip in 3-byte mode so a host MCU booting from it keeps working.
    async fn exit_4b(&mut self, info: &FlashInfo) -> Result<(), Error<D::Error>> {
        if info.addr_bytes == 4 {
            self.write_enable().await?;
            self.cmd(op::EXIT_4B).await?;
        }
        Ok(())
    }

    fn check_range(info: &FlashInfo, addr: u32, len: usize) -> Result<(), Error<D::Error>> {
        let end = u64::from(addr) + len as u64;
        if end > u64::from(info.capacity) {
            return Err(Error::OutOfRange);
        }
        Ok(())
    }

    pub async fn read(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), Error<D::Error>> {
        let info = self.info_or_err()?;
        Self::check_range(&info, addr, buf.len())?;
        self.enter_4b(&info).await?;
        let mut hdr = [0u8; 5];
        let n = Self::header(op::READ, addr, info.addr_bytes, &mut hdr);
        let r = self
            .dev
            .transaction(&mut [Operation::Write(&hdr[..n]), Operation::Read(buf)])
            .await
            .map_err(Error::Spi);
        self.exit_4b(&info).await?;
        r
    }

    /// Plans the erase of `[addr, addr + len)`; execute with [`Flash::erase_op`].
    pub fn plan_erase(&self, addr: u32, len: u32) -> Result<ErasePlan, Error<D::Error>> {
        let info = self.info_or_err()?;
        Ok(ErasePlan::new(&info, addr, len)?)
    }

    pub async fn erase_op(&mut self, e: EraseOp) -> Result<(), Error<D::Error>> {
        let info = self.info_or_err()?;
        self.enter_4b(&info).await?;
        self.write_enable().await?;
        let mut hdr = [0u8; 5];
        let n = Self::header(e.opcode, e.addr, info.addr_bytes, &mut hdr);
        let r = match self.dev.write(&hdr[..n]).await {
            Ok(()) => self.wait_ready(5_000, ERASE_TIMEOUT_MS).await,
            Err(err) => Err(Error::Spi(err)),
        };
        self.exit_4b(&info).await?;
        r
    }

    /// Erases a range completely (convenience wrapper over plan + ops).
    pub async fn erase(&mut self, addr: u32, len: u32) -> Result<(), Error<D::Error>> {
        for e in self.plan_erase(addr, len)? {
            self.erase_op(e).await?;
        }
        Ok(())
    }

    /// Programs `data` at `addr`, splitting at page boundaries. The area must be erased.
    pub async fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), Error<D::Error>> {
        let info = self.info_or_err()?;
        Self::check_range(&info, addr, data.len())?;
        self.enter_4b(&info).await?;
        let mut result = Ok(());
        for (a, off, n) in page_chunks(addr, data.len(), info.page_size) {
            result = self.program_page(&info, a, &data[off..off + n]).await;
            if result.is_err() {
                break;
            }
        }
        self.exit_4b(&info).await?;
        result
    }

    async fn program_page(
        &mut self,
        info: &FlashInfo,
        addr: u32,
        chunk: &[u8],
    ) -> Result<(), Error<D::Error>> {
        self.write_enable().await?;
        let mut hdr = [0u8; 5];
        let n = Self::header(op::PAGE_PROGRAM, addr, info.addr_bytes, &mut hdr);
        self.dev
            .transaction(&mut [Operation::Write(&hdr[..n]), Operation::Write(chunk)])
            .await
            .map_err(Error::Spi)?;
        self.wait_ready(50, PROGRAM_TIMEOUT_MS).await
    }
}

#[cfg(test)]
mod tests;
