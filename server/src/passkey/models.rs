#![allow(non_snake_case)]

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::cbor::{read_bytes, read_int};

pub const PASSKEYS_FILE: &str = "/etc/kvm/passkeys.json";
pub const RECOVERY_CODES_FILE: &str = "/etc/kvm/recovery_codes.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyCredential {
    pub id: String,
    pub public_key: CoseKey,
    pub counter: u32,
    pub transports: Vec<String>,
    pub created: DateTime<Utc>,
    pub device_name: Option<String>,
    pub rp_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoseKey {
    pub kty: u8,
    pub alg: i32,
    pub n: Vec<u8>,
    pub e: Vec<u8>,
    pub crv: Option<u8>,
}

impl CoseKey {
    pub fn from_cbor(data: &[u8]) -> Option<Self> {
        if data.is_empty() || data[0] != 0xa1 {
            return None;
        }

        let mut offset = 1;
        let mut map = std::collections::HashMap::new();

        while offset < data.len() {
            let key = read_int(data, &mut offset)?;
            let value = read_bytes(data, &mut offset)?;
            map.insert(key, value);
        }

        let kty = map.get(&1).cloned().unwrap_or_default();
        let alg = map.get(&3).cloned().unwrap_or_default();
        let kty_val = kty.first().copied().unwrap_or(0);
        let alg_val = if alg.len() >= 4 {
            i32::from_be_bytes([alg[0], alg[1], alg[2], alg[3]])
        } else {
            0
        };

        let n = map.get(&-1).cloned().unwrap_or_default();
        let e = map.get(&-2).cloned().unwrap_or_default();
        let crv = map.get(&-1).and_then(|v| v.first().copied());

        Some(CoseKey {
            kty: kty_val,
            alg: alg_val,
            n,
            e,
            crv,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyStorage {
    pub credentials: Vec<PasskeyCredential>,
    pub updated_at: DateTime<Utc>,
}

impl Default for PasskeyStorage {
    fn default() -> Self {
        Self {
            credentials: Vec::new(),
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCode {
    pub code: String,
    pub used: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryStorage {
    pub codes: Vec<RecoveryCode>,
    pub created: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyResponse {
    pub id: String,
    pub rawId: String,
    #[serde(rename = "type")]
    pub type_field: String,
    pub response: AttestationResponse,
    pub clientExtensionResults: ClientExtensionResults,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttestationResponse {
    pub clientDataJSON: String,
    pub attestationObject: Option<String>,
    pub authenticatorData: Option<String>,
    pub signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientExtensionResults {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginChallengeResponse {
    pub challenge: String,
    pub challenge_id: String,
    pub rp_id: String,
    pub timeout: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupResponse {
    pub success: bool,
    pub funnel_url: String,
    pub enrollment_url: String,
    pub qr_code: String,
    pub expires_at: String,
    #[serde(rename = "challengeId", skip_serializing_if = "String::is_empty")]
    pub challenge_id: String,
    #[serde(rename = "setupToken", skip_serializing_if = "String::is_empty")]
    pub setup_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyResponse {
    pub success: bool,
    pub token: Option<String>,
    pub requires_password_change: Option<bool>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverResponse {
    pub success: bool,
    pub token: Option<String>,
    pub remaining_codes: Option<u32>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryCodesResponse {
    pub success: bool,
    pub codes: Vec<String>,
}
