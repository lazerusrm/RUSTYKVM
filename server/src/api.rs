use serde::Serialize;

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
