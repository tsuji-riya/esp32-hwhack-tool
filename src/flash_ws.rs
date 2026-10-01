//! SPI flash access over a WebSocket.
//!
//! Text frames carry commands (browser -> device) and JSON replies (device -> browser),
//! binary frames carry flash data.
//!
//! ```text
//! hello                              -> {"type":"hello",...}
//! probe <dev> <hz>                   -> {"type":"probe",...} | {"type":"error",...}
//! set <dev> <capacity> <addr_bytes>  -> same as probe, with manual parameters
//! status <dev>                       -> {"type":"status","sr":N}
//! unprotect <dev>                    -> {"type":"done","op":"unprotect"}
//! read <dev> <addr> <len>            -> binary frames..., {"type":"done","op":"read"}
//! erase <dev> <addr> <len>           -> {"type":"progress"}..., {"type":"done","op":"erase"}
//! write <dev> <addr> <len>           -> {"type":"ready"}; send binary frames, each answered with progress
//! ```

use alloc::vec;
use core::fmt;

use flash_core::{Error as FlashError, FlashInfo, InfoSource, info::manufacturer_name};
use picoserve::futures::Either;
use picoserve::io::{Read, Write};
use picoserve::response::ws::{Message, SocketRx, SocketTx, WebSocketCallback};

use crate::config::{FLASH_CHUNK, FLASH_DEVICES, SPI_DEFAULT_HZ, SPI_MAX_HZ};
use crate::flash_hub::{HUB, Hub, flash};

pub struct FlashWs;

enum Next {
    Continue,
    Close(Option<(u16, &'static str)>),
}

#[derive(Clone, Copy)]
enum Cmd {
    Hello,
    Probe {
        dev: usize,
        hz: u32,
    },
    Set {
        dev: usize,
        capacity: u32,
        addr_bytes: u8,
    },
    Status {
        dev: usize,
    },
    Unprotect {
        dev: usize,
    },
    Read {
        dev: usize,
        addr: u32,
        len: u32,
    },
    Erase {
        dev: usize,
        addr: u32,
        len: u32,
    },
    Write {
        dev: usize,
        addr: u32,
        len: u32,
    },
    Invalid,
}

fn parse(line: &str) -> Cmd {
    /// Decimal or `0x` hexadecimal number.
    fn num<T: TryFrom<u32>>(it: &mut core::str::SplitWhitespace<'_>) -> Option<T> {
        let s = it.next()?;
        let v = match s.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => s.parse::<u32>().ok()?,
        };
        T::try_from(v).ok()
    }

    let mut it = line.split_whitespace();
    let Some(word) = it.next() else {
        return Cmd::Invalid;
    };
    let it = &mut it;
    let parsed = (|| {
        Some(match word {
            "hello" => Cmd::Hello,
            "probe" => Cmd::Probe {
                dev: num(it)?,
                hz: num(it).unwrap_or(SPI_DEFAULT_HZ),
            },
            "set" => Cmd::Set {
                dev: num(it)?,
                capacity: num(it)?,
                addr_bytes: num(it)?,
            },
            "status" => Cmd::Status { dev: num(it)? },
            "unprotect" => Cmd::Unprotect { dev: num(it)? },
            "read" => Cmd::Read {
                dev: num(it)?,
                addr: num(it)?,
                len: num(it)?,
            },
            "erase" => Cmd::Erase {
                dev: num(it)?,
                addr: num(it)?,
                len: num(it)?,
            },
            "write" => Cmd::Write {
                dev: num(it)?,
                addr: num(it)?,
                len: num(it)?,
            },
            _ => return None,
        })
    })();
    parsed.unwrap_or(Cmd::Invalid)
}

fn flash_error_text<E>(e: &FlashError<E>) -> &'static str {
    match e {
        FlashError::Spi(_) => "SPI bus error",
        FlashError::NoDevice => "no flash answering (check wiring/power; JEDEC ID is 00 or FF)",
        FlashError::NotProbed => "chip not probed yet: run probe first",
        FlashError::Plan(flash_core::PlanError::NoEraseTypes) => "chip reports no erase commands",
        FlashError::Plan(flash_core::PlanError::Unaligned { .. }) => {
            "address/length not aligned to the erase size"
        }
        FlashError::Plan(flash_core::PlanError::OutOfRange) | FlashError::OutOfRange => {
            "range exceeds chip capacity"
        }
        FlashError::Timeout => "timeout waiting for the chip (write protected or busy)",
    }
}

struct ProbeJson<'a>(&'a FlashInfo, u8);

impl fmt::Display for ProbeJson<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let i = self.0;
        write!(
            f,
            "{{\"type\":\"probe\",\"jedec\":\"{:02X}{:02X}{:02X}\",\"vendor\":\"{}\",\"capacity\":{},\"addrBytes\":{},\"pageSize\":{},\"source\":\"{}\",\"sr\":{},\"erase\":[",
            i.jedec[0],
            i.jedec[1],
            i.jedec[2],
            manufacturer_name(i.jedec[0]).unwrap_or("unknown"),
            i.capacity,
            i.addr_bytes,
            i.page_size,
            match i.source {
                InfoSource::Sfdp => "sfdp",
                InfoSource::Default => "default",
                InfoSource::Manual => "manual",
            },
            self.1,
        )?;
        for (n, e) in i.erase_types().enumerate() {
            if n > 0 {
                f.write_str(",")?;
            }
            write!(f, "{{\"op\":{},\"size\":{}}}", e.opcode, e.size)?;
        }
        f.write_str("]}")
    }
}

async fn send_error<W: Write>(tx: &mut SocketTx<W>, msg: &str) -> Result<(), W::Error> {
    tx.send_display(format_args!("{{\"type\":\"error\",\"msg\":\"{msg}\"}}"))
        .await
}

async fn send_done<W: Write>(tx: &mut SocketTx<W>, op: &str) -> Result<(), W::Error> {
    tx.send_display(format_args!("{{\"type\":\"done\",\"op\":\"{op}\"}}"))
        .await
}

async fn send_progress<W: Write>(
    tx: &mut SocketTx<W>,
    op: &str,
    done: u32,
    total: u32,
) -> Result<(), W::Error> {
    tx.send_display(format_args!(
        "{{\"type\":\"progress\",\"op\":\"{op}\",\"done\":{done},\"total\":{total}}}"
    ))
    .await
}

impl WebSocketCallback for FlashWs {
    async fn run<R: Read, W: Write<Error = R::Error>>(
        self,
        mut rx: SocketRx<R>,
        mut tx: SocketTx<W>,
    ) -> Result<(), W::Error> {
        let mut msg = vec![0u8; FLASH_CHUNK].into_boxed_slice();
        let mut data = vec![0u8; FLASH_CHUNK].into_boxed_slice();

        loop {
            let event = rx
                .next_message(
                    &mut msg,
                    core::future::pending::<core::convert::Infallible>(),
                )
                .await?;
            let cmd = match event {
                Either::First(Ok(Message::Text(line))) => parse(line),
                Either::First(Ok(Message::Binary(_))) => Cmd::Invalid,
                Either::First(Ok(Message::Ping(p))) => {
                    tx.send_pong(p).await?;
                    continue;
                }
                Either::First(Ok(Message::Pong(_))) => continue,
                Either::First(Ok(Message::Close(_))) => return tx.close(None).await,
                Either::First(Err(e)) => return tx.close((e.code(), "protocol error")).await,
                Either::Second(never) => match never {},
            };

            match cmd {
                Cmd::Hello => {
                    tx.send_display(format_args!(
                        "{{\"type\":\"hello\",\"devices\":{FLASH_DEVICES},\"maxHz\":{SPI_MAX_HZ},\"defaultHz\":{SPI_DEFAULT_HZ},\"chunk\":{FLASH_CHUNK}}}"
                    ))
                    .await?
                }
                Cmd::Invalid => send_error(&mut tx, "bad command").await?,
                Cmd::Write { dev, addr, len } => {
                    if let Next::Close(reason) = handle_write(&mut rx, &mut tx, &mut msg, dev, addr, len).await? {
                        return tx.close(reason).await;
                    }
                }
                other => {
                    let Ok(mut guard) = HUB.try_lock() else {
                        send_error(&mut tx, "SPI bus busy").await?;
                        continue;
                    };
                    let Some(hub) = guard.as_mut() else {
                        send_error(&mut tx, "SPI not initialised").await?;
                        continue;
                    };
                    handle(hub, other, &mut tx, &mut data).await?;
                }
            }
        }
    }
}

fn dev_ok(dev: usize) -> bool {
    dev < FLASH_DEVICES
}

/// All commands except `write` (which needs to read frames from the browser).
async fn handle<W: Write>(
    hub: &mut Hub,
    cmd: Cmd,
    tx: &mut SocketTx<W>,
    data: &mut [u8],
) -> Result<(), W::Error> {
    match cmd {
        Cmd::Probe { dev, hz } if dev_ok(dev) => {
            if hub.set_hz(hz).is_err() {
                return send_error(tx, "unsupported SPI frequency").await;
            }
            let mut f = flash(&mut hub.spi, &mut hub.cs[dev]);
            match f.probe().await {
                Ok(info) => {
                    let sr = f.read_status().await.unwrap_or(0);
                    hub.infos[dev] = Some(info);
                    tx.send_display(ProbeJson(&info, sr)).await
                }
                Err(e) => send_error(tx, flash_error_text(&e)).await,
            }
        }
        Cmd::Set {
            dev,
            capacity,
            addr_bytes,
        } if dev_ok(dev) => {
            if !(addr_bytes == 3 || addr_bytes == 4) || capacity == 0 {
                return send_error(tx, "capacity must be > 0 and addr_bytes 3 or 4").await;
            }
            let jedec = hub.infos[dev].map_or([0; 3], |i| i.jedec);
            let info = FlashInfo::manual(jedec, capacity, addr_bytes);
            hub.infos[dev] = Some(info);
            tx.send_display(ProbeJson(&info, 0)).await
        }
        Cmd::Status { dev } if dev_ok(dev) => {
            let mut f = flash(&mut hub.spi, &mut hub.cs[dev]);
            match f.read_status().await {
                Ok(sr) => {
                    tx.send_display(format_args!("{{\"type\":\"status\",\"sr\":{sr}}}"))
                        .await
                }
                Err(e) => send_error(tx, flash_error_text(&e)).await,
            }
        }
        Cmd::Unprotect { dev } if dev_ok(dev) => {
            let mut f = flash(&mut hub.spi, &mut hub.cs[dev]);
            match f.clear_protection().await {
                Ok(()) => send_done(tx, "unprotect").await,
                Err(e) => send_error(tx, flash_error_text(&e)).await,
            }
        }
        Cmd::Read { dev, addr, len } if dev_ok(dev) => {
            let Some(info) = hub.infos[dev] else {
                return send_error(tx, "chip not probed yet: run probe first").await;
            };
            let mut f = flash(&mut hub.spi, &mut hub.cs[dev]);
            f.set_info(info);
            let mut done = 0u32;
            while done < len {
                let n = (len - done).min(data.len() as u32) as usize;
                match f.read(addr + done, &mut data[..n]).await {
                    Ok(()) => tx.send_binary(&data[..n]).await?,
                    Err(e) => return send_error(tx, flash_error_text(&e)).await,
                }
                done += n as u32;
            }
            send_done(tx, "read").await
        }
        Cmd::Erase { dev, addr, len } if dev_ok(dev) => {
            let Some(info) = hub.infos[dev] else {
                return send_error(tx, "chip not probed yet: run probe first").await;
            };
            let mut f = flash(&mut hub.spi, &mut hub.cs[dev]);
            f.set_info(info);
            let plan = match f.plan_erase(addr, len) {
                Ok(p) => p,
                Err(e) => return send_error(tx, flash_error_text(&e)).await,
            };
            let mut done = 0u32;
            for op in plan {
                if let Err(e) = f.erase_op(op).await {
                    return send_error(tx, flash_error_text(&e)).await;
                }
                done += op.size;
                send_progress(tx, "erase", done, len).await?;
            }
            send_done(tx, "erase").await
        }
        _ => send_error(tx, "invalid device index").await,
    }
}

/// Returns [`Next::Close`] when the connection must be closed.
async fn handle_write<R: Read, W: Write<Error = R::Error>>(
    rx: &mut SocketRx<R>,
    tx: &mut SocketTx<W>,
    msg: &mut [u8],
    dev: usize,
    addr: u32,
    len: u32,
) -> Result<Next, W::Error> {
    if !dev_ok(dev) {
        send_error(tx, "invalid device index").await?;
        return Ok(Next::Continue);
    }
    let Ok(mut guard) = HUB.try_lock() else {
        send_error(tx, "SPI bus busy").await?;
        return Ok(Next::Continue);
    };
    let Some(hub) = guard.as_mut() else {
        send_error(tx, "SPI not initialised").await?;
        return Ok(Next::Continue);
    };
    let Some(info) = hub.infos[dev] else {
        send_error(tx, "chip not probed yet: run probe first").await?;
        return Ok(Next::Continue);
    };
    if u64::from(addr) + u64::from(len) > u64::from(info.capacity) {
        send_error(tx, "range exceeds chip capacity").await?;
        return Ok(Next::Continue);
    }
    let mut f = flash(&mut hub.spi, &mut hub.cs[dev]);
    f.set_info(info);

    tx.send_text("{\"type\":\"ready\"}").await?;

    let mut done = 0u32;
    while done < len {
        let event = rx
            .next_message(msg, core::future::pending::<core::convert::Infallible>())
            .await?;
        match event {
            Either::First(Ok(Message::Binary(chunk))) => {
                if u64::from(done) + chunk.len() as u64 > u64::from(len) {
                    send_error(tx, "more data than announced").await?;
                    return Ok(Next::Continue);
                }
                if let Err(e) = f.program(addr + done, chunk).await {
                    send_error(tx, flash_error_text(&e)).await?;
                    return Ok(Next::Continue);
                }
                done += chunk.len() as u32;
                send_progress(tx, "write", done, len).await?;
            }
            Either::First(Ok(Message::Text(_))) => {
                send_error(tx, "write aborted").await?;
                return Ok(Next::Continue);
            }
            Either::First(Ok(Message::Ping(p))) => tx.send_pong(p).await?,
            Either::First(Ok(Message::Pong(_))) => {}
            Either::First(Ok(Message::Close(_))) => return Ok(Next::Close(None)),
            Either::First(Err(e)) => return Ok(Next::Close(Some((e.code(), "protocol error")))),
            Either::Second(never) => match never {},
        }
    }
    send_done(tx, "write").await?;
    Ok(Next::Continue)
}
