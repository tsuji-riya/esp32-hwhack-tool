//! Browser <-> target UART bridge over a binary WebSocket.
//!
//! * binary frames: raw bytes (browser -> UART TX, UART RX -> browser)
//! * text frames from the browser: `baud <rate> [8N1]` to reconfigure the port

use alloc::vec;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use esp_hal::Async;
use esp_hal::uart::{Config, DataBits, Parity, StopBits, Uart};
use picoserve::futures::Either;
use picoserve::io::{Read, Write};
use picoserve::response::ws::{Message, SocketRx, SocketTx, WebSocketCallback};

/// UART port wrapper that may live in a `static`.
pub struct UartPort(pub Uart<'static, Async>);

// SAFETY: only used from tasks of the single embassy executor, behind `UART`'s mutex.
unsafe impl Send for UartPort {}

pub static UART: Mutex<CriticalSectionRawMutex, Option<UartPort>> = Mutex::new(None);

const MSG_BUF: usize = 1024;
const RX_BUF: usize = 512;

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
        let Ok(mut guard) = UART.try_lock() else {
            tx.send_text("busy: UART bridge already in use").await?;
            return tx.close((1013, "busy")).await;
        };
        let Some(UartPort(uart)) = guard.as_mut() else {
            return tx.close((1011, "uart unavailable")).await;
        };

        let mut msg = vec![0u8; MSG_BUF].into_boxed_slice();
        let mut rx_buf = vec![0u8; RX_BUF].into_boxed_slice();

        loop {
            let event = rx
                .next_message(&mut msg, uart.read_async(&mut rx_buf))
                .await?;
            match event {
                Either::First(Ok(Message::Binary(data))) => {
                    let mut data = data;
                    while !data.is_empty() {
                        match uart.write_async(data).await {
                            Ok(n) => data = &data[n..],
                            Err(_) => break,
                        }
                    }
                }
                Either::First(Ok(Message::Text(cmd))) => match parse_config(cmd) {
                    Some(cfg) => match uart.apply_config(&cfg) {
                        Ok(()) => tx.send_text("ok").await?,
                        Err(_) => tx.send_text("error: unsupported uart config").await?,
                    },
                    None => tx.send_text("error: bad command").await?,
                },
                Either::First(Ok(Message::Ping(p))) => tx.send_pong(p).await?,
                Either::First(Ok(Message::Pong(_))) => {}
                Either::First(Ok(Message::Close(_))) => return tx.close(None).await,
                Either::First(Err(e)) => return tx.close((e.code(), "protocol error")).await,
                Either::Second(Ok(n)) => tx.send_binary(&rx_buf[..n]).await?,
                // FIFO overflow / framing error: tell the browser but keep going.
                Either::Second(Err(_)) => tx.send_text("warn: uart rx error").await?,
            }
        }
    }
}
