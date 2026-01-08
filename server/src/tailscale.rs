use axum::http::StatusCode;
use axum::{extract::Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::{error, info};

const SCRIPT_PATH: &str = "/etc/init.d/S98tailscaled";
const SCRIPT_BACKUP_PATH: &str = "/kvmapp/system/init.d/S98tailscaled";
const TAILSCALE_PATH: &str = "/usr/bin/tailscale";
const TAILSCALED_PATH: &str = "/usr/sbin/tailscaled";
const ORIGINAL_URL: &str = "https://pkgs.tailscale.com/stable/tailscale_latest_riscv64.tgz";
const WORKSPACE: &str = "/root/.tailscale";

#[derive(Serialize)]
pub struct TsStatus {
    #[serde(rename = "BackendState")]
    pub backend_state: String,
    #[serde(rename = "Self")]
    pub self_node: TsSelfNode,
    #[serde(rename = "CurrentTailnet")]
    pub current_tailnet: TsTailnet,
}

#[derive(Serialize)]
pub struct TsSelfNode {
    #[serde(rename = "HostName")]
    pub host_name: String,
    #[serde(rename = "TailscaleIPs")]
    pub tailscale_ips: Vec<String>,
}

#[derive(Serialize)]
pub struct TsTailnet {
    #[serde(rename = "Name")]
    pub name: String,
}

#[derive(Serialize)]
pub struct LoginRsp {
    pub url: String,
}

pub async fn tailscale_start_handler() -> impl IntoResponse {
    let cmd = format!(
        "cp -f {} {} && {} start",
        SCRIPT_BACKUP_PATH, SCRIPT_PATH, SCRIPT_PATH
    );
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn tailscale_stop_handler() -> impl IntoResponse {
    let cmd = format!("{} stop && rm -f {}", SCRIPT_PATH, SCRIPT_PATH);
    match Command::new("sh").arg("-c").arg(cmd).status().await {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn tailscale_status_handler() -> impl IntoResponse {
    let output = Command::new("sh")
        .arg("-c")
        .arg("tailscale status --json")
        .output()
        .await;
    match output {
        Ok(out) => {
            let s = String::from_utf8_lossy(&out.stdout);
            if let Some(idx) = s.find('{') {
                return (StatusCode::OK, s[idx..].to_string()).into_response();
            }
            (StatusCode::INTERNAL_SERVER_ERROR, "Invalid JSON").into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

pub async fn tailscale_login_handler() -> impl IntoResponse {
    // We use tokio::process::Command with a timeout
    let cmd_future = Command::new("sh")
        .arg("-c")
        .arg("tailscale login --accept-dns=false --timeout=30s")
        .output();

    match tokio::time::timeout(std::time::Duration::from_secs(35), cmd_future).await {
        Ok(Ok(out)) => {
            // Tailscale login URL can be in stderr or stdout
            let err_s = String::from_utf8_lossy(&out.stderr);
            let out_s = String::from_utf8_lossy(&out.stdout);

            let combined = format!("{}{}", err_s, out_s);
            for line in combined.lines() {
                if line.contains("https://") {
                    if let Some(url_idx) = line.find("https://") {
                        let url = line[url_idx..]
                            .split_whitespace()
                            .next()
                            .unwrap_or_default();
                        if url.starts_with("https://login.tailscale.com") {
                            return Json(LoginRsp {
                                url: url.to_string(),
                            })
                            .into_response();
                        }
                    }
                }
            }
            error!("Tailscale login URL not found in output: {}", combined);
            (StatusCode::INTERNAL_SERVER_ERROR, "No login URL found").into_response()
        }
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, "Tailscale command timed out").into_response(),
    }
}

pub async fn tailscale_up_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg("tailscale up --accept-dns=false")
        .status()
        .await
    {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn tailscale_down_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg("tailscale down")
        .status()
        .await
    {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn tailscale_logout_handler() -> impl IntoResponse {
    match Command::new("sh")
        .arg("-c")
        .arg("tailscale logout")
        .status()
        .await
    {
        Ok(s) if s.success() => StatusCode::OK,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

pub async fn tailscale_install_handler() -> impl IntoResponse {
    tokio::spawn(async move {
        if let Err(e) = perform_tailscale_install().await {
            error!("Tailscale installation failed: {}", e);
        }
    });
    StatusCode::ACCEPTED
}

pub async fn tailscale_uninstall_handler() -> impl IntoResponse {
    let _ = Command::new("sh")
        .arg("-c")
        .arg(format!("{} stop", SCRIPT_PATH))
        .status()
        .await;
    let _ = tokio::fs::remove_file(TAILSCALE_PATH).await;
    let _ = tokio::fs::remove_file(TAILSCALED_PATH).await;
    let _ = tokio::fs::remove_file(SCRIPT_PATH).await;
    StatusCode::OK
}

async fn perform_tailscale_install() -> anyhow::Result<()> {
    let _ = tokio::fs::create_dir_all(WORKSPACE).await;
    let tar_file = format!("{}/tailscale.tgz", WORKSPACE);

    // 1. Download
    let resp = reqwest::get(ORIGINAL_URL).await?;
    let bytes = resp.bytes().await?;
    tokio::fs::write(&tar_file, &bytes).await?;

    // 2. Extract (using spawn_blocking for long synchronous operation)
    let tar_file_clone = tar_file.clone();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(&tar_file_clone)?;
        let tar = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(tar);
        archive.unpack(WORKSPACE)
    })
    .await??;

    // 3. Move binaries (finding them in the extracted dir)
    let mut entries = tokio::fs::read_dir(WORKSPACE).await?;
    while let Some(entry) = entries.next_entry().await? {
        if entry.file_type().await?.is_dir() {
            let dir_path = entry.path();
            let ts = dir_path.join("tailscale");
            let tsd = dir_path.join("tailscaled");
            if ts.exists() && tsd.exists() {
                tokio::fs::copy(&ts, TAILSCALE_PATH).await?;
                tokio::fs::copy(&tsd, TAILSCALED_PATH).await?;
                let _ = Command::new("chmod")
                    .arg("755")
                    .arg(TAILSCALE_PATH)
                    .status()
                    .await;
                let _ = Command::new("chmod")
                    .arg("755")
                    .arg(TAILSCALED_PATH)
                    .status()
                    .await;
                break;
            }
        }
    }

    let _ = tokio::fs::remove_dir_all(WORKSPACE).await;
    info!("Tailscale installed successfully");
    Ok(())
}
