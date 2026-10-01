//! Wi-Fi station: joins the configured network and gets an address via DHCP.

use embassy_net::{Runner, Stack};
use embassy_time::{Duration, Timer};
use esp_radio::wifi::{
    AuthenticationMethod, Config, Interface, WifiController, sta::StationConfig,
};

use crate::config::{WIFI_PASSWORD, WIFI_SSID};

pub fn configure_station(controller: &mut WifiController<'static>) {
    let mut cfg = StationConfig::default().with_ssid(WIFI_SSID);
    if !WIFI_PASSWORD.is_empty() {
        cfg = cfg
            .with_auth_method(AuthenticationMethod::Wpa2Personal)
            .with_password(WIFI_PASSWORD.into());
    }
    controller
        .set_config(&Config::Station(cfg))
        .expect("wifi config");
}

/// Connects and reconnects forever; also owns the controller so the radio stays up.
#[embassy_executor::task]
pub async fn connection_task(mut controller: WifiController<'static>) -> ! {
    loop {
        defmt::info!("wifi: connecting to '{}'...", WIFI_SSID);
        match controller.connect_async().await {
            Ok(_) => {
                defmt::info!("wifi: connected");
                // Returns when the link drops (router reboot, out of range, ...).
                let _ = controller.wait_for_disconnect_async().await;
                defmt::warn!("wifi: disconnected");
            }
            Err(e) => defmt::warn!("wifi: connect failed: {}", defmt::Debug2Format(&e)),
        }
        Timer::after(Duration::from_secs(3)).await;
    }
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, Interface<'static>>) -> ! {
    runner.run().await
}

/// Logs the URL to open each time a DHCP lease is obtained.
#[embassy_executor::task]
pub async fn report_task(stack: Stack<'static>) {
    loop {
        stack.wait_config_up().await;
        if let Some(cfg) = stack.config_v4() {
            defmt::info!(
                "Open http://{}/ in your browser",
                defmt::Display2Format(&cfg.address.address())
            );
        }
        stack.wait_config_down().await;
    }
}
