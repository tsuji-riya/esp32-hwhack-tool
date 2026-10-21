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
/// Number of concurrent HTTP/WebSocket connections (the UART WebSocket + page loads).
pub const WEB_TASKS: usize = 4;

pub const UART_DEFAULT_BAUD: u32 = 115_200;

/// Number of chip-select lines wired for SPI flash targets (see `bin/main.rs`).
pub const FLASH_DEVICES: usize = 3;
pub const SPI_DEFAULT_HZ: u32 = 4_000_000;
/// READ (0x03) has no dummy cycle: many chips spec it at 20–33 MHz, and the ESP32's
/// full-duplex input delay corrupts bits well before 40 MHz.
pub const SPI_MAX_HZ: u32 = 20_000_000;
/// TCP port of the flashrom serprog server (`flashrom -p serprog:ip=<IP>:8888`).
pub const SERPROG_PORT: u16 = 8888;
