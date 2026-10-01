//! Build-time configuration. Pin assignments live in `bin/main.rs`.

/// Wi-Fi network the ESP32 joins (station mode). Set at build time:
/// `HWHACK_SSID=MyNetwork HWHACK_PASSWORD=secret cargo run --release`
/// An empty password means an open network.
pub const WIFI_SSID: &str = match option_env!("HWHACK_SSID") {
    Some(s) => s,
    None => "",
};
pub const WIFI_PASSWORD: &str = match option_env!("HWHACK_PASSWORD") {
    Some(s) => s,
    None => "",
};

pub const HTTP_PORT: u16 = 80;
/// Number of concurrent HTTP/WebSocket connections (two WebSockets + page loads).
pub const WEB_TASKS: usize = 4;

pub const UART_DEFAULT_BAUD: u32 = 115_200;

/// Number of chip-select lines wired for SPI flash targets (see `bin/main.rs`).
pub const FLASH_DEVICES: usize = 3;
pub const SPI_DEFAULT_HZ: u32 = 4_000_000;
pub const SPI_MAX_HZ: u32 = 40_000_000;
/// Bytes moved per WebSocket data frame.
pub const FLASH_CHUNK: usize = 4096;
