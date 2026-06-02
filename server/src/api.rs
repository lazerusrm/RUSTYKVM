use serde::Serialize;

/// Standard error codes (aligned with common Go server conventions where applicable).
/// Code 0 = success. Negative values = errors.
pub mod error_codes {
    /// Generic / unexpected error
    pub const GENERIC: i32 = -1;

    /// Authentication / authorization failure
    pub const AUTH: i32 = -2;

    /// Validation / bad request error
    pub const VALIDATION: i32 = -3;

    /// Resource not found
    pub const NOT_FOUND: i32 = -4;

    /// Account / IP locked (brute force protection)
    pub const LOCKED: i32 = -5;

    /// Operation not supported on this platform or configuration
    pub const NOT_SUPPORTED: i32 = -6;

    /// Hardware / device error (e.g. HDMI not ready, no signal)
    pub const HARDWARE: i32 = -7;

    /// Storage / image related error
    pub const STORAGE: i32 = -8;

    /// Network configuration error
    pub const NETWORK: i32 = -9;

    /// Script execution error
    pub const SCRIPT: i32 = -10;
}

/// API response wrapper matching the Go server format:
/// `{ "code": 0, "msg": "success", "data": ... }`
///
/// Notes:
/// - Most endpoints return HTTP 200 even on failure, and signal failure via `code != 0`.
/// - `data` is always present in JSON (null when `None`), matching Go's `interface{}` behavior.
#[derive(Debug, Serialize)]
pub struct ApiResponse<T: Serialize> {
    pub code: i32,
    pub msg: String,
    pub data: Option<T>,
}

impl<T: Serialize> ApiResponse<T> {
    pub fn ok(data: T) -> Self {
        Self {
            code: 0,
            msg: "success".to_string(),
            data: Some(data),
        }
    }

    pub fn err(code: i32, msg: &str) -> Self {
        Self {
            code,
            msg: msg.to_string(),
            data: None,
        }
    }
}

impl ApiResponse<serde_json::Value> {
    /// For endpoints that conceptually return no data, return an empty object.
    ///
    /// This is more tolerant of frontend code that mistakenly checks `!rsp.data`.
    pub fn ok_empty() -> Self {
        Self::ok(serde_json::json!({}))
    }
}
