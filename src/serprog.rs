//! flashrom `serprog` programmer over TCP: `flashrom -p serprog:ip=<IP>:8888`.
//!
//! Implements the SPI subset of flashrom's serprog protocol (Documentation/serprog-protocol.txt).
//! One client at a time; all multi-byte fields are little-endian.

use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embedded_io_async::{Read, Write};
use esp_hal::Async;
use esp_hal::gpio::Output;
use esp_hal::spi::master::{Config, Spi};
use esp_hal::time::Rate;

use crate::config::{FLASH_DEVICES, SERPROG_PORT, SPI_DEFAULT_HZ, SPI_MAX_HZ};

const ACK: u8 = 0x06;
const NAK: u8 = 0x15;

const NOP: u8 = 0x00;
const Q_IFACE: u8 = 0x01;
const Q_CMDMAP: u8 = 0x02;
const Q_PGMNAME: u8 = 0x03;
const Q_SERBUF: u8 = 0x04;
const Q_BUSTYPE: u8 = 0x05;
const Q_WRNMAXLEN: u8 = 0x08;
const SYNCNOP: u8 = 0x10;
const Q_RDNMAXLEN: u8 = 0x11;
const S_BUSTYPE: u8 = 0x12;
const O_SPIOP: u8 = 0x13;
const S_SPI_FREQ: u8 = 0x14;
const S_SPI_CS: u8 = 0x16;

const SUPPORTED: [u8; 13] = [
    NOP,
    Q_IFACE,
    Q_CMDMAP,
    Q_PGMNAME,
    Q_SERBUF,
    Q_BUSTYPE,
    Q_WRNMAXLEN,
    SYNCNOP,
    Q_RDNMAXLEN,
    S_BUSTYPE,
    O_SPIOP,
    S_SPI_FREQ,
    S_SPI_CS,
];
const BUS_SPI: u8 = 1 << 3;

/// Largest data payload of one SPI op in each direction (flashrom chunks to these).
const WRITE_MAX: usize = 256;
const READ_MAX: usize = 4096;
/// Opcode + 4-byte address + dummy bytes on top of the data.
const WRITE_OVERHEAD: usize = 8;

/// flashrom sends nothing while it waits on the user; a dead peer frees the port after this.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

pub struct Programmer {
    pub spi: Spi<'static, Async>,
    pub cs: [Output<'static>; FLASH_DEVICES],
}

struct Session<'a> {
    p: &'a mut Programmer,
    cs: usize,
    wbuf: [u8; WRITE_MAX + WRITE_OVERHEAD],
    rbuf: [u8; READ_MAX],
}

const CMDMAP: [u8; 32] = {
    let mut map = [0u8; 32];
    let mut i = 0;
    while i < SUPPORTED.len() {
        map[(SUPPORTED[i] / 8) as usize] |= 1 << (SUPPORTED[i] % 8);
        i += 1;
    }
    map
};
// Bit n of byte n/8 marks command n as supported.
const _: () = assert!(CMDMAP[0] == 0b0011_1111 && CMDMAP[1] == 0b0000_0001);
const _: () = assert!(CMDMAP[2] == 0b0101_1111 && CMDMAP[3] == 0);

fn u24(b: &[u8]) -> usize {
    usize::from(b[0]) | usize::from(b[1]) << 8 | usize::from(b[2]) << 16
}

impl Programmer {
    /// Returns the clock actually applied (clamped to what the wiring handles).
    fn set_hz(&mut self, hz: u32) -> Option<u32> {
        let hz = hz.clamp(100_000, SPI_MAX_HZ);
        self.spi
            .apply_config(
                &Config::default()
                    .with_frequency(Rate::from_hz(hz))
                    .with_mode(esp_hal::spi::Mode::_0),
            )
            .ok()?;
        Some(hz)
    }
}

impl Session<'_> {
    async fn spiop(&mut self, slen: usize, rlen: usize) -> Result<(), esp_hal::spi::Error> {
        use embedded_hal_async::spi::SpiBus;
        let Programmer { spi, cs } = &mut *self.p;
        let (wbuf, rbuf) = (&self.wbuf[..slen], &mut self.rbuf[..rlen]);
        let pin = &mut cs[self.cs];
        pin.set_low();
        let r = async {
            SpiBus::write(spi, wbuf).await?;
            SpiBus::read(spi, rbuf).await?;
            SpiBus::flush(spi).await
        }
        .await;
        pin.set_high();
        r
    }

    /// Handles one command. `Err` means the stream is out of sync or gone: drop the client.
    async fn command(&mut self, sock: &mut TcpSocket<'_>, cmd: u8) -> Result<(), ()> {
        let mut arg = [0u8; 6];
        match cmd {
            NOP => sock.write_all(&[ACK]).await,
            SYNCNOP => sock.write_all(&[NAK, ACK]).await,
            Q_IFACE => sock.write_all(&[ACK, 1, 0]).await,
            Q_CMDMAP => {
                sock.write_all(&[ACK]).await.map_err(drop)?;
                sock.write_all(&CMDMAP).await
            }
            Q_PGMNAME => sock.write_all(b"\x06esp32-hwhack\0\0\0\0").await,
            // Only used by flashrom for parallel-bus streaming; report the TCP receive buffer.
            Q_SERBUF => sock.write_all(&[ACK, 0x00, 0x04]).await,
            Q_BUSTYPE => sock.write_all(&[ACK, BUS_SPI]).await,
            Q_WRNMAXLEN | Q_RDNMAXLEN => {
                let max = if cmd == Q_WRNMAXLEN {
                    WRITE_MAX
                } else {
                    READ_MAX
                };
                let [a, b, c, _] = (max as u32).to_le_bytes();
                sock.write_all(&[ACK, a, b, c]).await
            }
            S_BUSTYPE => {
                sock.read_exact(&mut arg[..1]).await.map_err(drop)?;
                let ok = arg[0] & BUS_SPI != 0;
                sock.write_all(&[if ok { ACK } else { NAK }]).await
            }
            S_SPI_CS => {
                sock.read_exact(&mut arg[..1]).await.map_err(drop)?;
                let ok = usize::from(arg[0]) < FLASH_DEVICES;
                if ok {
                    self.cs = usize::from(arg[0]);
                }
                sock.write_all(&[if ok { ACK } else { NAK }]).await
            }
            S_SPI_FREQ => {
                sock.read_exact(&mut arg[..4]).await.map_err(drop)?;
                let req = u32::from_le_bytes([arg[0], arg[1], arg[2], arg[3]]);
                let hz = if req == 0 { None } else { self.p.set_hz(req) };
                match hz {
                    Some(hz) => {
                        let [a, b, c, d] = hz.to_le_bytes();
                        sock.write_all(&[ACK, a, b, c, d]).await
                    }
                    None => sock.write_all(&[NAK]).await,
                }
            }
            O_SPIOP => {
                sock.read_exact(&mut arg).await.map_err(drop)?;
                let (slen, rlen) = (u24(&arg[..3]), u24(&arg[3..]));
                if slen > self.wbuf.len() || rlen > READ_MAX {
                    // Larger than we advertised: the payload can't be skipped safely.
                    defmt::warn!("serprog: spiop too large ({}/{})", slen, rlen);
                    return Err(());
                }
                sock.read_exact(&mut self.wbuf[..slen])
                    .await
                    .map_err(drop)?;
                match self.spiop(slen, rlen).await {
                    Ok(()) => {
                        sock.write_all(&[ACK]).await.map_err(drop)?;
                        sock.write_all(&self.rbuf[..rlen]).await
                    }
                    Err(_) => sock.write_all(&[NAK]).await,
                }
            }
            _ => sock.write_all(&[NAK]).await,
        }
        .map_err(drop)?;
        sock.flush().await.map_err(drop)
    }
}

#[embassy_executor::task]
pub async fn serprog_task(stack: Stack<'static>, mut programmer: Programmer) -> ! {
    let mut rx = [0u8; 1024];
    let mut tx = [0u8; READ_MAX + 64];
    loop {
        let mut sock = TcpSocket::new(stack, &mut rx, &mut tx);
        sock.set_timeout(Some(IDLE_TIMEOUT));
        // Every command is a request/response round trip; don't let Nagle hold the tail.
        sock.set_nagle_enabled(false);
        if sock.accept(SERPROG_PORT).await.is_err() {
            continue;
        }
        defmt::info!("serprog: client connected");
        // Each client starts from the power-on clock and CS0, like a fresh serial programmer.
        let _ = programmer.set_hz(SPI_DEFAULT_HZ);
        let mut s = Session {
            p: &mut programmer,
            cs: 0,
            wbuf: [0; WRITE_MAX + WRITE_OVERHEAD],
            rbuf: [0; READ_MAX],
        };
        let mut cmd = [0u8; 1];
        while sock.read_exact(&mut cmd).await.is_ok() {
            if s.command(&mut sock, cmd[0]).await.is_err() {
                break;
            }
        }
        sock.abort();
        let _ = sock.flush().await;
        defmt::info!("serprog: client disconnected");
    }
}
