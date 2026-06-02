//! Serial port discovery and authenticated WebSocket attach (Go terminal uses picocom; this API is for direct attach).

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::response::IntoResponse;
use axum::Json;
use futures::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::SerialPortBuilderExt;
use tracing::{error, info};

use crate::api::ApiResponse;

#[derive(Debug, Serialize)]
pub struct SerialPortInfo {
    pub path: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct GetSerialPortsRsp {
    pub ports: Vec<SerialPortInfo>,
}

pub async fn get_serial_ports_handler() -> impl IntoResponse {
    let mut ports = Vec::new();
    let patterns = ["/dev/ttyS", "/dev/ttyUSB", "/dev/ttyACM"];

    if let Ok(entries) = std::fs::read_dir("/dev") {
        for entry in entries.flatten() {
            if let Some(p) = entry.path().to_str() {
                for prefix in &patterns {
                    if p.starts_with(prefix) {
                        let desc = if p.contains("USB") || p.contains("ACM") {
                            "USB Serial".to_string()
                        } else {
                            "Onboard UART".to_string()
                        };
                        ports.push(SerialPortInfo {
                            path: p.to_string(),
                            description: desc,
                        });
                        break;
                    }
                }
            }
        }
    }
    ports.sort_by(|a, b| a.path.cmp(&b.path));
    Json(ApiResponse::ok(GetSerialPortsRsp { ports }))
}

#[derive(Debug, Deserialize)]
pub struct SerialQuery {
    #[serde(default = "default_baud")]
    pub baud: u32,
}

fn default_baud() -> u32 {
    115200
}

/// `port` is a catch-all path segment (URL-encoded `/dev/ttyS1` → `/dev/ttyS1`).
pub async fn serial_ws_handler(
    ws: WebSocketUpgrade,
    Path(port): Path<String>,
    Query(query): Query<SerialQuery>,
) -> impl IntoResponse {
    let baud = query.baud;
    ws.on_upgrade(move |socket| handle_serial_socket(socket, port, baud))
}

async fn handle_serial_socket(mut socket: WebSocket, port: String, baud: u32) {
    let port = if port.starts_with('/') {
        port
    } else {
        format!("/{}", port)
    };

    if !port.starts_with("/dev/tty") || port.contains("..") {
        let _ = socket.send(Message::Text("Invalid port".into())).await;
        return;
    }

    info!("Opening serial port {} @ {} baud", port, baud);

    let mut serial = match tokio_serial::new(&port, baud)
        .timeout(std::time::Duration::from_millis(100))
        .open_native_async()
    {
        Ok(s) => s,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("Open failed: {}", e).into()))
                .await;
            return;
        }
    };

    let mut read_buf = [0u8; 1024];
    loop {
        tokio::select! {
            res = serial.read(&mut read_buf) => {
                match res {
                    Ok(0) => break,
                    Ok(n) => {
                        if socket
                            .send(Message::Binary(read_buf[..n].to_vec().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) => {
                        error!("serial read error on {}: {}", port, e);
                        break;
                    }
                }
            }
            msg = socket.next() => {
                match msg {
                    Some(Ok(Message::Binary(data))) => {
                        if serial.write_all(&data).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Text(text))) => {
                        if serial.write_all(text.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Err(e)) => {
                        error!("serial websocket error: {}", e);
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}
