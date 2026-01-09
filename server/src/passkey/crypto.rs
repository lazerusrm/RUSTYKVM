use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct AuthenticatorData {
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub counter: u32,
    pub attested_credential_included: bool,
    pub extensions: Vec<u8>,
}

impl AuthenticatorData {
    pub fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < 37 {
            return None;
        }

        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&data[0..32]);

        let flags = data[32];
        let mut counter = [0u8; 4];
        counter.copy_from_slice(&data[33..37]);
        let counter = u32::from_be_bytes(counter);

        let attested_credential_included = (flags & 0x40) != 0;

        let extensions = if (flags & 0x80) != 0 && data.len() > 37 {
            let ext_len = data[37] as usize;
            if 38 + ext_len <= data.len() {
                data[38..38 + ext_len].to_vec()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        Some(AuthenticatorData {
            rp_id_hash,
            flags,
            counter,
            attested_credential_included,
            extensions,
        })
    }
}

#[derive(Debug)]
pub struct ClientData {
    pub type_: String,
    pub challenge: String,
    pub origin: String,
    pub cross_origin: bool,
}

impl ClientData {
    pub fn parse(json: &str) -> Option<Self> {
        let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(json).ok()?;

        let type_ = parsed
            .get("type")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())?;

        let challenge = parsed
            .get("challenge")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())?;

        let origin = parsed
            .get("origin")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())?;

        let cross_origin = parsed
            .get("crossOrigin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Some(ClientData {
            type_,
            challenge,
            origin,
            cross_origin,
        })
    }

    pub fn hash(&self) -> Vec<u8> {
        let json = format!(
            r#"{{"type":"{}","challenge":"{}","origin":"{}","crossOrigin":{}}}"#,
            self.type_, self.challenge, self.origin, self.cross_origin
        );
        Sha256::digest(json.as_bytes()).to_vec()
    }
}

use crate::passkey::models::CoseKey;

impl CoseKey {
    pub fn verify_signature(&self, message: &[u8], signature: &[u8]) -> bool {
        match self.kty {
            1 => {
                if self.alg != -257 && self.alg != -259 {
                    return false;
                }

                let key_size = self.n.len();
                let e_len = self.e.len();

                let mut asn1 = Vec::with_capacity(key_size + e_len + 4 + 2);
                asn1.push(0x30);
                asn1.push((key_size + e_len + 4 + 2 - 2) as u8);
                asn1.push(0x02);
                asn1.push(e_len as u8);
                asn1.extend_from_slice(&self.e);
                asn1.push(0x02);
                asn1.push(key_size as u8);
                asn1.extend_from_slice(&self.n);

                let verifier = ring::signature::UnparsedPublicKey::new(
                    &ring::signature::RSA_PKCS1_2048_8192_SHA256,
                    asn1,
                );
                verifier.verify(message, signature).is_ok()
            }
            2 => {
                if self.n.is_empty() || self.n.len() < 65 {
                    return false;
                }

                let x = &self.n[1..33];
                let y = &self.n[33..65];

                let mut point = vec![0x04];
                point.extend_from_slice(x);
                point.extend_from_slice(y);

                let alg = match self.crv {
                    Some(6) | Some(1) => &ring::signature::ECDSA_P256_SHA256_ASN1,
                    Some(8) => &ring::signature::ECDSA_P384_SHA384_ASN1,
                    _ => &ring::signature::ECDSA_P256_SHA256_ASN1,
                };

                let verifier = ring::signature::UnparsedPublicKey::new(alg, point);
                verifier.verify(message, signature).is_ok()
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_authenticator_data_parse() {
        let data = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e,
            0x0f, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c,
            0x1d, 0x1e, 0x1f, 0x20, 0x01, 0x00, 0x00, 0x00, 0x01,
        ];

        let auth_data = AuthenticatorData::parse(&data).unwrap();
        assert_eq!(auth_data.flags, 0x01);
        assert_eq!(auth_data.counter, 1);
    }

    #[test]
    fn test_client_data_parse() {
        let json = r#"{"type":"webauthn.get","challenge":"dGVzdA","origin":"https://example.com","crossOrigin":false}"#;
        let client_data = ClientData::parse(json).unwrap();
        assert_eq!(client_data.type_, "webauthn.get");
        assert_eq!(client_data.origin, "https://example.com");
    }
}
