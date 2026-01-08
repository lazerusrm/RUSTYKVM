use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

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
    pub fn from_cbor(cbor_data: &[u8]) -> Option<Self> {
        if cbor_data.is_empty() {
            return None;
        }

        fn read_int(data: &[u8], offset: &mut usize) -> Option<i64> {
            if *offset >= data.len() {
                return None;
            }
            let first = data[*offset];
            *offset += 1;
            match first {
                0..=23 => Some(first as i64),
                24 => {
                    if *offset >= data.len() { None } else {
                        let val = data[*offset] as i64;
                        *offset += 1;
                        Some(val)
                    }
                }
                25 => {
                    if *offset + 1 >= data.len() { None } else {
                        let val = u16::from_be_bytes([data[*offset], data[*offset + 1]]) as i64;
                        *offset += 2;
                        Some(val)
                    }
                }
                26 => {
                    if *offset + 3 >= data.len() { None } else {
                        let val = u32::from_be_bytes([data[*offset], data[*offset + 1], data[*offset + 2], data[*offset + 3]]) as i64;
                        *offset += 4;
                        Some(val)
                    }
                }
                27 => {
                    if *offset + 7 >= data.len() { None } else {
                        let val = u64::from_be_bytes([
                            data[*offset], data[*offset + 1], data[*offset + 2], data[*offset + 3],
                            data[*offset + 4], data[*offset + 5], data[*offset + 6], data[*offset + 7]
                        ]) as i64;
                        *offset += 8;
                        Some(val)
                    }
                }
                _ => None,
            }
        }

        fn read_bytes(data: &[u8], offset: &mut usize) -> Option<Vec<u8>> {
            if *offset >= data.len() {
                return None;
            }
            let first = data[*offset];
            *offset += 1;
            match first {
                0..=23 => {
                    let len = first as usize;
                    if *offset + len > data.len() { return None; }
                    let result = data[*offset..*offset + len].to_vec();
                    *offset += len;
                    Some(result)
                }
                64 => None,
                65 => {
                    if *offset >= data.len() { return None; }
                    let len = data[*offset] as usize;
                    *offset += 1;
                    if *offset + len > data.len() { return None; }
                    let result = data[*offset..*offset + len].to_vec();
                    *offset += len;
                    Some(result)
                }
                _ => None,
            }
        }

        let mut offset = 0;

        if data[offset] != 0xa1 {
            return None;
        }
        offset += 1;

        let mut map = std::collections::HashMap::new();

        while offset < data.len() {
            let key = match read_int(data, &mut offset) {
                Some(k) => k,
                None => break,
            };
            let value = match read_bytes(data, &mut offset) {
                Some(v) => v,
                None => break,
            };
            map.insert(key, value);

            if offset >= data.len() {
                break;
            }
        }

        let kty = map.get(&1).cloned().unwrap_or_default();
        let alg = map.get(&3).cloned().unwrap_or_default();
        let kty_val = if !kty.is_empty() { kty[0] as u8 } else { 0 };
        let alg_val = if !alg.is_empty() { i32::from_be_bytes([alg.get(0).copied().unwrap_or(0), alg.get(1).copied().unwrap_or(0), alg.get(2).copied().unwrap_or(0), alg.get(3).copied().unwrap_or(0)]) } else { 0 };

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
