pub mod models;
pub mod handlers;
pub mod recovery;
pub mod qr;

use std::sync::Arc;
use tokio::sync::Mutex;
use crate::AppState;

pub struct PasskeyState {
    pub pending_challenge: Mutex<Option<PendingChallenge>>,
}

#[derive(Debug)]
pub struct PendingChallenge {
    pub challenge_id: String,
    pub challenge: Vec<u8>,
    pub user_id: Vec<u8>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub is_enrollment: bool,
}

const CHALLENGE_TTL_MINUTES: i64 = 5;

impl PasskeyState {
    pub fn new() -> Self {
        Self {
            pending_challenge: Mutex::new(None),
        }
    }

    pub fn generate_challenge_id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    pub fn new_enrollment_challenge(&self, challenge_id: String, challenge: Vec<u8>, user_id: Vec<u8>) -> PendingChallenge {
        let now = chrono::Utc::now();
        PendingChallenge {
            challenge_id,
            challenge,
            user_id,
            created_at: now,
            expires_at: now + chrono::Duration::minutes(CHALLENGE_TTL_MINUTES),
            is_enrollment: true,
        }
    }

    pub fn new_login_challenge(&self, challenge_id: String, challenge: Vec<u8>) -> PendingChallenge {
        let now = chrono::Utc::now();
        PendingChallenge {
            challenge_id,
            challenge,
            user_id: Vec::new(),
            created_at: now,
            expires_at: now + chrono::Duration::minutes(CHALLENGE_TTL_MINUTES),
            is_enrollment: false,
        }
    }

    pub fn is_expired(&self) -> bool {
        let now = chrono::Utc::now();
        match &*self.pending_challenge.lock().await {
            Some(c) => now >= c.expires_at,
            None => true,
        }
    }
}

impl Default for PasskeyState {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn get_passkey_state(state: &Arc<AppState>) -> &PasskeyState {
    &state.passkey_state
}
