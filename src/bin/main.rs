#![no_std]
#![no_main]
#![recursion_limit = "512"]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use defmt::error;
use embassy_executor::Spawner;
use embassy_net::StackResources;
use embassy_time::{Duration, Timer};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::uart::{Config as UartConfig, Uart};
use esp_println as _;
use esp32_hwhack_tool::config::{SPI_DEFAULT_HZ, UART_DEFAULT_BAUD, WEB_TASKS, WIFI_SSID};
use esp32_hwhack_tool::serprog::{Programmer, serprog_task};
use esp32_hwhack_tool::uart_bridge::uart_task;
use esp32_hwhack_tool::{web, wifi};
use static_cell::StaticCell;

#[panic_handler]
fn panic(panic_info: &core::panic::PanicInfo) -> ! {
    error!("{}", panic_info);
    // Reboot instead of hanging, so the device comes back on its own.
    esp_hal::system::software_reset()
}

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

/// HTTP connections + serprog + DHCP client + DNS + headroom.
static STACK_RESOURCES: StaticCell<StackResources<{ WEB_TASKS + 5 }>> = StaticCell::new();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The Wi-Fi driver needs a heap. Part of it lives in RAM reclaimed from the bootloader.
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    // --- Target UART (UART0 stays free for the log output) ----------------------------------
    //   ESP32 GPIO25 (TX) -> target RX,  ESP32 GPIO26 (RX) <- target TX,  common GND.
    let uart = Uart::new(
        peripherals.UART1,
        UartConfig::default().with_baudrate(UART_DEFAULT_BAUD),
    )
    .expect("uart config")
    .with_tx(peripherals.GPIO25)
    .with_rx(peripherals.GPIO26)
    .into_async();
    spawner.spawn(uart_task(uart).unwrap());

    // --- Target SPI flash bus (VSPI pins) ---------------------------------------------------
    //   SCK GPIO18, MOSI GPIO23, MISO GPIO19, chip selects GPIO5 / GPIO21 / GPIO22.
    let spi = Spi::new(
        peripherals.SPI3,
        SpiConfig::default()
            .with_frequency(Rate::from_hz(SPI_DEFAULT_HZ))
            .with_mode(esp_hal::spi::Mode::_0),
    )
    .expect("spi config")
    .with_sck(peripherals.GPIO18)
    .with_mosi(peripherals.GPIO23)
    .with_miso(peripherals.GPIO19)
    .into_async();
    let programmer = Programmer {
        spi,
        cs: [
            Output::new(peripherals.GPIO5, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO21, Level::High, OutputConfig::default()),
            Output::new(peripherals.GPIO22, Level::High, OutputConfig::default()),
        ],
    };

    // --- Wi-Fi station + network stack -------------------------------------------------------
    if WIFI_SSID.is_empty() {
        error!(
            "No Wi-Fi network configured: build with HWHACK_SSID=<ssid> HWHACK_PASSWORD=<password>"
        );
    }
    let (mut controller, interfaces) =
        esp_radio::wifi::new(peripherals.WIFI, Default::default()).expect("wifi init");
    wifi::configure_station(&mut controller);

    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        interfaces.station,
        embassy_net::Config::dhcpv4(Default::default()),
        STACK_RESOURCES.init(StackResources::new()),
        seed,
    );

    spawner.spawn(wifi::connection_task(controller).unwrap());
    spawner.spawn(wifi::net_task(runner).unwrap());
    spawner.spawn(wifi::report_task(stack).unwrap());
    spawner.spawn(serprog_task(stack, programmer).unwrap());
    for id in 0..WEB_TASKS {
        spawner.spawn(web::web_task(id, stack).unwrap());
    }

    loop {
        Timer::after(Duration::from_secs(60)).await;
    }
}
