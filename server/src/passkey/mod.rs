#![allow(dead_code)]

pub mod cbor;
pub mod crypto;
pub mod handlers;
pub mod models;
pub mod qr;
pub mod recovery;

use tokio::sync::Mutex;

pub struct PasskeyState {
    pub pending_challenge: Mutex<Option<PendingChallenge>>,
    /// Recovery codes from the last completed enrollment (shown once to authenticated admin).
    pub pending_recovery_codes: Mutex<Option<Vec<String>>>,
}

#[derive(Debug, Clone)]
pub struct PendingChallenge {
    pub challenge_id: String,
    pub challenge: String,
    pub user_id: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub is_enrollment: bool,
    pub rp_id: String,
    pub credential_id: Option<String>,
    /// Secret issued by authenticated setup; required to complete enrollment.
    pub setup_token: String,
}

const CHALLENGE_TTL_MINUTES: i64 = 5;

impl PendingChallenge {
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now() >= self.expires_at
    }
}

impl PasskeyState {
    pub fn new() -> Self {
        Self {
            pending_challenge: Mutex::new(None),
            pending_recovery_codes: Mutex::new(None),
        }
    }

    pub fn generate_challenge_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    pub fn new_enrollment_challenge(
        &self,
        challenge_id: String,
        challenge: String,
        user_id: Vec<u8>,
        rp_id: String,
        credential_id: Option<String>,
        setup_token: String,
    ) -> PendingChallenge {
        let now = chrono::Utc::now();
        PendingChallenge {
            challenge_id,
            challenge,
            user_id,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(CHALLENGE_TTL_MINUTES),
            is_enrollment: true,
            rp_id,
            credential_id,
            setup_token,
        }
    }

    pub fn new_login_challenge(
        &self,
        challenge_id: String,
        challenge: String,
        rp_id: String,
        credential_id: Option<String>,
    ) -> PendingChallenge {
        let now = chrono::Utc::now();
        PendingChallenge {
            challenge_id,
            challenge,
            user_id: Vec::new(),
            created_at: now,
            expires_at: now + chrono::Duration::minutes(CHALLENGE_TTL_MINUTES),
            is_enrollment: false,
            rp_id,
            credential_id,
            setup_token: String::new(),
        }
    }
}

impl Default for PasskeyState {
    fn default() -> Self {
        Self::new()
    }
}
