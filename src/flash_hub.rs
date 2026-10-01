//! Shared SPI bus with several chip-select lines, exposed as `SpiDevice`s to `flash-core`.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Delay;
use embedded_hal_async::spi::{ErrorType, Operation, SpiBus, SpiDevice};
use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::spi::master::{Config, Spi};
use esp_hal::time::Rate;
use flash_core::{Flash, FlashInfo};

use crate::config::FLASH_DEVICES;

pub struct Hub {
    pub spi: Spi<'static, Async>,
    pub cs: [Output<'static>; FLASH_DEVICES],
    /// Last detected (or manually set) info per chip-select line.
    pub infos: [Option<FlashInfo>; FLASH_DEVICES],
    pub hz: u32,
}

// SAFETY: the hub is only ever touched from tasks of the single embassy executor
// (always behind `HUB`'s mutex); `esp-hal` marks `Async` peripherals `!Send` merely to
// keep them on the core that owns their interrupts.
unsafe impl Send for Hub {}

pub static HUB: Mutex<CriticalSectionRawMutex, Option<Hub>> = Mutex::new(None);

impl Hub {
    pub fn set_hz(&mut self, hz: u32) -> Result<(), esp_hal::spi::master::ConfigError> {
        let hz = hz.clamp(100_000, crate::config::SPI_MAX_HZ);
        self.spi.apply_config(
            &Config::default()
                .with_frequency(Rate::from_hz(hz))
                .with_mode(esp_hal::spi::Mode::_0),
        )?;
        self.hz = hz;
        Ok(())
    }
}

/// One chip-select line on the shared bus.
pub struct HubDevice<'a> {
    spi: &'a mut Spi<'static, Async>,
    cs: &'a mut Output<'static>,
}

impl ErrorType for HubDevice<'_> {
    type Error = esp_hal::spi::Error;
}

impl SpiDevice for HubDevice<'_> {
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        self.cs.set_low();
        let result = async {
            for op in operations.iter_mut() {
                match op {
                    Operation::Read(buf) => SpiBus::read(&mut *self.spi, buf).await?,
                    Operation::Write(buf) => SpiBus::write(&mut *self.spi, buf).await?,
                    Operation::Transfer(read, write) => {
                        SpiBus::transfer(&mut *self.spi, read, write).await?
                    }
                    Operation::TransferInPlace(buf) => {
                        SpiBus::transfer_in_place(&mut *self.spi, buf).await?
                    }
                    Operation::DelayNs(_) => {}
                }
            }
            SpiBus::flush(&mut *self.spi).await
        }
        .await;
        self.cs.set_high();
        result
    }
}

pub type HubFlash<'a> = Flash<HubDevice<'a>, Delay>;

/// Builds a flash driver for one chip-select. Takes the hub fields separately so the
/// caller can keep using `Hub::infos` while the driver is alive.
pub fn flash<'a>(spi: &'a mut Spi<'static, Async>, cs: &'a mut Output<'static>) -> HubFlash<'a> {
    Flash::new(HubDevice { spi, cs }, Delay)
}
