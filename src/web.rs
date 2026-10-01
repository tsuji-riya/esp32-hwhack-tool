//! HTTP server: serves the React UI (embedded in flash) and the two WebSocket endpoints.

use embassy_net::Stack;
use embassy_time::Duration;
use picoserve::response::ws::WebSocketUpgrade;
use picoserve::routing::{PathRouter, get, parse_path_segment};
use picoserve::{Router, Timeouts};

use crate::config::{HTTP_PORT, WEB_TASKS};
use crate::flash_ws::FlashWs;
use crate::uart_bridge::UartWs;

include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

struct Asset {
    mime: &'static str,
    gzip: bool,
    etag: &'static str,
    body: &'static [u8],
}

fn find(name: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|(n, _)| *n == name).map(|(_, a)| a)
}

fn respond(asset: &'static Asset) -> impl picoserve::response::IntoResponse {
    use picoserve::response::Response;
    let headers: [(&str, &str); 3] = [
        (
            "Content-Encoding",
            if asset.gzip { "gzip" } else { "identity" },
        ),
        ("ETag", asset.etag),
        ("Cache-Control", "no-cache"),
    ];
    Response::ok(asset.body)
        .with_content_type(asset.mime)
        .with_headers(headers)
}

async fn index() -> impl picoserve::response::IntoResponse {
    respond(find("index.html").expect("index.html is always embedded"))
}

async fn asset(
    name: heapless::String<64>,
) -> Result<impl picoserve::response::IntoResponse, (picoserve::response::StatusCode, &'static str)>
{
    match find(&name) {
        Some(a) => Ok(respond(a)),
        // Unknown paths fall back to the SPA entry point.
        None => Err((picoserve::response::StatusCode::NOT_FOUND, "not found")),
    }
}

fn make_app() -> Router<impl PathRouter> {
    Router::new()
        .route("/", get(index))
        .route(
            "/ws/uart",
            get(|upgrade: WebSocketUpgrade| async move { upgrade.on_upgrade(UartWs) }),
        )
        .route(
            "/ws/flash",
            get(|upgrade: WebSocketUpgrade| async move { upgrade.on_upgrade(FlashWs) }),
        )
        .route(
            ("/", parse_path_segment::<heapless::String<64>>()),
            get(asset),
        )
}

#[embassy_executor::task(pool_size = WEB_TASKS)]
pub async fn web_task(id: usize, stack: Stack<'static>) -> ! {
    let app = make_app();
    let config = picoserve::Config::new(Timeouts {
        start_read_request: Duration::from_secs(10),
        persistent_start_read_request: Duration::from_secs(5),
        read_request: Duration::from_secs(5),
        write: Duration::from_secs(10),
    })
    .keep_connection_alive();

    let mut tcp_rx = [0u8; 2048];
    let mut tcp_tx = [0u8; 4096];
    let mut http = [0u8; 2048];

    picoserve::Server::new(&app, &config, &mut http)
        .listen_and_serve(id, stack, HTTP_PORT, &mut tcp_rx, &mut tcp_tx)
        .await
        .into_never()
}
