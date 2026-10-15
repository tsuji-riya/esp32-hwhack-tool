//! Browser <-> target UART bridge over a binary WebSocket.
//!
//! * binary frames: raw bytes (browser -> UART TX, UART RX -> browser)
//! * text frames from the browser: `baud <rate> [8N1]` to reconfigure the port,
//!   `ping` (answered with `pong`) as a keep-alive
//!
//! `uart_task` owns the UART and drains its 128-byte RX FIFO into `RX_PIPE` at all times,
//! so a slow Wi-Fi send no longer overflows the hardware FIFO.

use core::ops::Range;
use core::sync::atomic::{AtomicBool, Ordering};

use alloc::vec;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_sync::pipe::Pipe;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use esp_hal::Async;
use esp_hal::uart::{Config, DataBits, Parity, StopBits, Uart};
use picoserve::futures::Either as WsEither;
use picoserve::io::{Read, Write};
use picoserve::response::ws::{Message, SocketRx, SocketTx, WebSocketCallback};

type Raw = CriticalSectionRawMutex;

const MSG_BUF: usize = 1024;
const RX_CHUNK: usize = 512;
const TX_CHUNK: usize = 256;
/// Close the session when the browser sends nothing (it pings every 5 s) for this long,
/// so a vanished client does not hold the bridge forever.
const IDLE_TIMEOUT: Duration = Duration::from_secs(20);

/// UART RX -> WebSocket. Sized for ~0.7 s at 115200 baud of Wi-Fi stall.
static RX_PIPE: Pipe<Raw, 8192> = Pipe::new();
/// WebSocket -> UART TX.
static TX_PIPE: Pipe<Raw, 1024> = Pipe::new();
static CONFIG: Signal<Raw, Config> = Signal::new();
static CONFIG_DONE: Signal<Raw, bool> = Signal::new();
/// Set when RX bytes were dropped (FIFO overflow, framing error or full `RX_PIPE`).
static LOST: AtomicBool = AtomicBool::new(false);
/// Held by the one active WebSocket session.
static SESSION: Mutex<Raw, ()> = Mutex::new(());

#[embassy_executor::task]
pub async fn uart_task(mut uart: Uart<'static, Async>) -> ! {
    let mut rx = [0u8; RX_CHUNK];
    let mut tx = [0u8; TX_CHUNK];
    let mut pending: Range<usize> = 0..0;
    loop {
        // Only push what fits in the TX FIFO right now, so RX is never left unattended.
        while !pending.is_empty() && uart.write_ready() {
            match uart.write(&tx[pending.clone()]) {
                Ok(n) => pending.start += n,
                Err(_) => pending = 0..0,
            }
        }
        let refill = async {
            if pending.is_empty() {
                Some(TX_PIPE.read(&mut tx).await)
            } else {
                // shortcut: polls TX FIFO space every 1 ms (UartTx can't be awaited while RX is borrowed).
                Timer::after_millis(1).await;
                None
            }
        };
        match select3(uart.read_async(&mut rx), CONFIG.wait(), refill).await {
            Either3::First(Ok(n)) => {
                if RX_PIPE.try_write(&rx[..n]).map_or(true, |w| w < n) {
                    LOST.store(true, Ordering::Relaxed);
                }
            }
            Either3::First(Err(_)) => LOST.store(true, Ordering::Relaxed),
            Either3::Second(cfg) => CONFIG_DONE.signal(uart.apply_config(&cfg).is_ok()),
            Either3::Third(Some(n)) => pending = 0..n,
            Either3::Third(None) => {}
        }
    }
}

pub struct UartWs;

/// Parses `baud <rate> [<bits><parity><stop>]`, e.g. `baud 115200 8N1`.
fn parse_config(cmd: &str) -> Option<Config> {
    let mut it = cmd.split_whitespace();
    if it.next()? != "baud" {
        return None;
    }
    let baud: u32 = it.next()?.parse().ok()?;
    let mut cfg = Config::default().with_baudrate(baud);
    if let Some(fmt) = it.next() {
        let b = fmt.as_bytes();
        if b.len() != 3 {
            return None;
        }
        cfg = cfg
            .with_data_bits(match b[0] {
                b'5' => DataBits::_5,
                b'6' => DataBits::_6,
                b'7' => DataBits::_7,
                b'8' => DataBits::_8,
                _ => return None,
            })
            .with_parity(match b[1].to_ascii_uppercase() {
                b'N' => Parity::None,
                b'E' => Parity::Even,
                b'O' => Parity::Odd,
                _ => return None,
            })
            .with_stop_bits(match b[2] {
                b'1' => StopBits::_1,
                b'2' => StopBits::_2,
                _ => return None,
            });
    }
    Some(cfg)
}

impl WebSocketCallback for UartWs {
    async fn run<R: Read, W: Write<Error = R::Error>>(
        self,
        mut rx: SocketRx<R>,
        mut tx: SocketTx<W>,
    ) -> Result<(), W::Error> {
        let Ok(_session) = SESSION.try_lock() else {
            tx.send_text("busy: UART bridge already in use").await?;
            return tx.close((1013, "busy")).await;
        };
        // Start from a clean slate rather than replaying the previous session's leftovers.
        RX_PIPE.clear();
        TX_PIPE.clear();
        LOST.store(false, Ordering::Relaxed);

        let mut msg = vec![0u8; MSG_BUF].into_boxed_slice();
        let mut rx_buf = vec![0u8; RX_CHUNK].into_boxed_slice();
        let mut deadline = Instant::now() + IDLE_TIMEOUT;

        loop {
            let signal = select(RX_PIPE.read(&mut rx_buf), Timer::at(deadline));
            let event = rx.next_message(&mut msg, signal).await?;
            if let WsEither::First(_) = event {
                deadline = Instant::now() + IDLE_TIMEOUT;
            }
            match event {
                WsEither::First(Ok(Message::Binary(data))) => TX_PIPE.write_all(data).await,
                WsEither::First(Ok(Message::Text("ping"))) => tx.send_text("pong").await?,
                WsEither::First(Ok(Message::Text(cmd))) => match parse_config(cmd) {
                    Some(cfg) => {
                        CONFIG_DONE.reset();
                        CONFIG.signal(cfg);
                        if CONFIG_DONE.wait().await {
                            tx.send_text("ok").await?
                        } else {
                            tx.send_text("error: unsupported uart config").await?
                        }
                    }
                    None => tx.send_text("error: bad command").await?,
                },
                WsEither::First(Ok(Message::Ping(p))) => tx.send_pong(p).await?,
                WsEither::First(Ok(Message::Pong(_))) => {}
                WsEither::First(Ok(Message::Close(_))) => return tx.close(None).await,
                WsEither::First(Err(e)) => return tx.close((e.code(), "protocol error")).await,
                WsEither::Second(Either::First(n)) => {
                    tx.send_binary(&rx_buf[..n]).await?;
                    if LOST.swap(false, Ordering::Relaxed) {
                        tx.send_text("warn: uart rx data lost").await?;
                    }
                }
                WsEither::Second(Either::Second(())) => {
                    return tx.close((1001, "idle timeout")).await;
                }
            }
        }
    }
}
