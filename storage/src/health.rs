use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::time::{timeout, Duration as TokioDuration};
use tracing::warn;

const HEALTH_CACHE_FILE: &str = "/var/lib/nanokvm/sd_health.json";
const HEALTH_CACHE_TTL_HOURS: i64 = 24;
const MAX_SYSFS_READ_MS: u64 = 1000;
const MAX_IO_ERRORS_THRESHOLD: u32 = 50;
const WARNING_IO_ERRORS_THRESHOLD: u32 = 10;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HealthStatus {
    Good,
    Fair,
    Warning,
    Fail,
    Unknown,
}

impl std::fmt::Display for HealthStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HealthStatus::Good => write!(f, "GOOD"),
            HealthStatus::Fair => write!(f, "FAIR"),
            HealthStatus::Warning => write!(f, "WARNING"),
            HealthStatus::Fail => write!(f, "FAIL"),
            HealthStatus::Unknown => write!(f, "UNKNOWN"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum EolStatus {
    Normal,
    PreEolWarning,
    EndOfLife,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdHealth {
    pub status: HealthStatus,
    pub wear_level: Option<u8>,
    pub pre_eol_status: Option<EolStatus>,
    pub io_errors: u32,
    pub temperature: Option<i32>,
    pub power_cycles: Option<u32>,
    pub read_count: u64,
    pub write_count: u64,
    pub capacity_bytes: u64,
    pub health_score: u8,
    pub last_check: DateTime<Utc>,
    pub next_check: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedHealth {
    pub health: SdHealth,
    pub cached_at: DateTime<Utc>,
}

async fn read_sysfs_file(path: &str) -> Option<String> {
    let timeout_duration = TokioDuration::from_millis(MAX_SYSFS_READ_MS);
    match timeout(timeout_duration, fs::read_to_string(path)).await {
        Ok(Ok(content)) => Some(content.trim().to_string()),
        _ => None,
    }
}

async fn read_u64_from_file(path: &str) -> Option<u64> {
    read_sysfs_file(path).await?.parse::<u64>().ok()
}

async fn read_u32_from_file(path: &str) -> Option<u32> {
    read_sysfs_file(path).await?.parse::<u32>().ok()
}

async fn read_i32_from_file(path: &str) -> Option<i32> {
    read_sysfs_file(path).await?.parse::<i32>().ok()
}

async fn find_mmc_device_path(_base: &str) -> Option<String> {
    let paths = [
        "/sys/class/mmc_host/mmc0/mmc0:0001",
        "/sys/class/mmc_host/mmc0/mmc0:0002",
        "/sys/class/mmc_host/mmc0/mmc0:0003",
    ];

    for path in &paths {
        if let Ok(true) = tokio::fs::try_exists(path).await {
            return Some(path.to_string());
        }
    }
    None
}

async fn read_wear_level(mmc_path: &str) -> Option<u8> {
    let life_path = format!("{}/life_time", mmc_path);
    let content = read_sysfs_file(&life_path).await?;

    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    match parts[0].parse::<u64>() {
        Ok(val) => Some(val as u8),
        Err(_) => None,
    }
}

async fn read_pre_eol_info(mmc_path: &str) -> Option<EolStatus> {
    let pre_eol_path = format!("{}/pre_eol_info", mmc_path);
    let content = read_sysfs_file(&pre_eol_path).await?;

    match content.as_str() {
        "1" | "01" | "normal" => Some(EolStatus::Normal),
        "2" | "02" | "pre-eol" | "warning" => Some(EolStatus::PreEolWarning),
        "3" | "03" | "eol" | "end of life" => Some(EolStatus::EndOfLife),
        _ => Some(EolStatus::Unknown),
    }
}

async fn read_temperature(mmc_path: &str) -> Option<i32> {
    let temp_paths = [
        format!("{}/temp", mmc_path),
        format!("{}/temperature", mmc_path),
    ];

    for path in &temp_paths {
        if let Some(temp) = read_i32_from_file(path).await {
            return Some(temp);
        }
    }
    None
}

async fn read_power_cycles(mmc_path: &str) -> Option<u32> {
    let cycles_path = format!("{}/power_cycles", mmc_path);
    read_u32_from_file(&cycles_path).await
}

async fn read_io_errors() -> (u32, u32) {
    let stat_path = "/sys/class/block/mmcblk0/device/stat";
    let content = match read_sysfs_file(stat_path).await {
        Some(c) => c,
        None => return (0, 0),
    };

    let parts: Vec<&str> = content.split_whitespace().collect();
    if parts.len() < 14 {
        return (0, 0);
    }

    let read_errors = parts
        .get(4)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);
    let write_errors = parts
        .get(10)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    (read_errors, write_errors)
}

async fn read_capacity() -> u64 {
    let size_path = "/sys/block/mmcblk0/size";
    let sectors = read_u64_from_file(size_path).await.unwrap_or_default();
    sectors * 512
}

async fn read_diskstats() -> (u64, u64) {
    let stat_path = "/proc/diskstats";
    let content = match read_sysfs_file(stat_path).await {
        Some(c) => c,
        None => return (0, 0),
    };

    for line in content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 14 && (parts[2] == "mmcblk0" || parts[3] == "mmcblk0") {
            let reads = parts[5].parse::<u64>().unwrap_or(0);
            let writes = parts[9].parse::<u64>().unwrap_or(0);
            return (reads, writes);
        }
    }
    (0, 0)
}

fn compute_health_score(
    wear_level: Option<u8>,
    io_errors: u32,
    temp: Option<i32>,
    pre_eol: Option<EolStatus>,
) -> u8 {
    let mut score = 100u8;

    if pre_eol == Some(EolStatus::EndOfLife) {
        return 0;
    }

    if pre_eol == Some(EolStatus::PreEolWarning) {
        score = score.saturating_sub(30);
    }

    if let Some(wear) = wear_level {
        if wear <= 10 {
            score = score.saturating_sub(50);
        } else if wear <= 30 {
            score = score.saturating_sub(25);
        } else if wear < 50 {
            score = score.saturating_sub(10);
        }
    }

    if io_errors > 100 {
        score = score.saturating_sub(40);
    } else if io_errors > 50 {
        score = score.saturating_sub(25);
    } else if io_errors > 10 {
        score = score.saturating_sub(10);
    } else if io_errors > 0 {
        score = score.saturating_sub(3);
    }

    if let Some(t) = temp {
        if t > 70 {
            score = score.saturating_sub(20);
        } else if t > 60 {
            score = score.saturating_sub(10);
        } else if t > 50 {
            score = score.saturating_sub(5);
        }
    }

    score.min(100)
}

fn assess_status(score: u8, io_errors: u32, pre_eol: Option<EolStatus>) -> HealthStatus {
    match pre_eol {
        Some(EolStatus::EndOfLife) => return HealthStatus::Fail,
        Some(EolStatus::PreEolWarning) => return HealthStatus::Warning,
        _ => {}
    }

    if io_errors > MAX_IO_ERRORS_THRESHOLD {
        return HealthStatus::Fail;
    }
    if io_errors > WARNING_IO_ERRORS_THRESHOLD {
        return HealthStatus::Warning;
    }

    match score {
        80..=100 => HealthStatus::Good,
        50..=79 => HealthStatus::Fair,
        20..=49 => HealthStatus::Warning,
        _ => HealthStatus::Fail,
    }
}

async fn load_cached_health() -> Option<CachedHealth> {
    match fs::read_to_string(HEALTH_CACHE_FILE).await {
        Ok(content) => serde_json::from_str(&content).ok(),
        Err(_) => None,
    }
}

async fn save_cached_health(health: &SdHealth) {
    if let Some(parent) = std::path::Path::new(HEALTH_CACHE_FILE).parent() {
        let _ = fs::create_dir_all(parent).await;
    }

    let cached = CachedHealth {
        health: health.clone(),
        cached_at: Utc::now(),
    };

    if let Ok(json) = serde_json::to_string(&cached) {
        let _ = fs::write(HEALTH_CACHE_FILE, json).await;
    }
}

async fn check_sysfs_health() -> SdHealth {
    let mmc_path = match find_mmc_device_path("/sys/class/mmc_host").await {
        Some(path) => path,
        None => {
            return SdHealth {
                status: HealthStatus::Unknown,
                wear_level: None,
                pre_eol_status: None,
                io_errors: 0,
                temperature: None,
                power_cycles: None,
                read_count: 0,
                write_count: 0,
                capacity_bytes: 0,
                health_score: 0,
                last_check: Utc::now(),
                next_check: Utc::now() + Duration::hours(HEALTH_CACHE_TTL_HOURS),
            };
        }
    };

    let wear_level = read_wear_level(&mmc_path).await;
    let pre_eol = read_pre_eol_info(&mmc_path).await;
    let temperature = read_temperature(&mmc_path).await;
    let power_cycles = read_power_cycles(&mmc_path).await;
    let (read_errs, write_errs) = read_io_errors().await;
    let io_errors = read_errs.saturating_add(write_errs);
    let (read_count, write_count) = read_diskstats().await;
    let capacity = read_capacity().await;

    let health_score = compute_health_score(wear_level, io_errors, temperature, pre_eol.clone());
    let status = assess_status(health_score, io_errors, pre_eol.clone());

    SdHealth {
        status,
        wear_level,
        pre_eol_status: pre_eol,
        io_errors,
        temperature,
        power_cycles,
        read_count,
        write_count,
        capacity_bytes: capacity,
        health_score,
        last_check: Utc::now(),
        next_check: Utc::now() + Duration::hours(HEALTH_CACHE_TTL_HOURS),
    }
}

pub async fn check_health() -> SdHealth {
    let now = Utc::now();

    if let Some(cached) = load_cached_health().await {
        let age = now.signed_duration_since(cached.cached_at);
        if age.num_hours() < HEALTH_CACHE_TTL_HOURS {
            return cached.health;
        }
    }

    let health = check_sysfs_health().await;
    save_cached_health(&health).await;

    health
}

pub async fn check_health_with_logging() -> SdHealth {
    let health = check_health().await;

    if let Some(cached) = load_cached_health().await {
        if cached.health.status != health.status {
            match health.status {
                HealthStatus::Warning | HealthStatus::Fail => {
                    warn!(
                        "SD card health: {} (was {})",
                        health.status, cached.health.status
                    );
                }
                _ => {}
            }
        }
    }

    health
}
