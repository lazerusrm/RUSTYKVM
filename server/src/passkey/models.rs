use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use chrono::{DateTime, Utc};

pub const PASSKEYS_FILE: &str = "/etc/kvm/passkeys.json";
pub const RECOVERY_CODES_FILE: &str = "/etc/kvm/recovery_codes.json";
pub const PENDING_FILE: &str = "/etc/kvm/passkey_pending.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyCredential {
    pub id: String,
    pub public_key: Vec<u8>,
    pub counter: u32,
    pub transports: Vec<String>,
    pub created: DateTime<Utc>,
    pub device_name: Option<String>,
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
pub struct PublicKeyCredentialDescriptor {
    #[serde(rename = "type")]
    pub type_field: String,
    pub id: String,
    pub transports: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialCreationOptions {
    pub publicKey: PublicKeyCredentialCreationOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKeyCredentialCreationOptions {
    pub rp: RelyingPartyEntity,
    pub user: UserEntity,
    pub challenge: String,
    pub pubKeyCredParams: Vec<PubKeyCredParam>,
    pub timeout: Option<u32>,
    pub excludeCredentials: Option<Vec<PublicKeyCredentialDescriptor>>,
    pub authenticatorSelection: Option<AuthenticatorSelection>,
    pub attestation: Option<String>,
    pub extensions: Option<CredentialCreationExtensions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelyingPartyEntity {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserEntity {
    pub id: String,
    pub name: String,
    pub displayName: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PubKeyCredParam {
    #[serde(rename = "type")]
    pub type_field: String,
    pub alg: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthenticatorSelection {
    pub authenticatorAttachment: Option<String>,
    pub residentKey: Option<String>,
    pub requireResidentKey: Option<bool>,
    pub userVerification: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialCreationExtensions {
    pub credProps: Option<CredPropsExtension>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredPropsExtension {
    pub rk: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRequestOptions {
    pub publicKey: PublicKeyCredentialRequestOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicKeyCredentialRequestOptions {
    pub challenge: String,
    pub timeout: Option<u32>,
    pub rpId: Option<String>,
    pub allowCredentials: Option<Vec<PublicKeyCredentialDescriptor>>,
    pub userVerification: Option<String>,
    pub extensions: Option<CredentialRequestExtensions>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialRequestExtensions {
    pub appid: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnrollmentStartResponse {
    pub challenge: String,
    pub challenge_id: String,
    pub user_id: String,
    pub rp_id: String,
    pub rp_name: String,
    pub user_name: String,
    pub user_display_name: String,
    pub timeout: u32,
}

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
