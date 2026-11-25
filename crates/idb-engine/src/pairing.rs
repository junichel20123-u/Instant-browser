//! Pairing a phone with this computer (remote control over the LAN).
//!
//! This module only creates and checks secrets; it opens no port. The LAN
//! listener (WebSocket + TLS) arrives with the daemon in phase 2 (ADR-0007).
//!
//! Flow: the computer shows an offer (a URI rendered as a QR code) with a
//! single-use code valid for 5 minutes and the TLS certificate fingerprint.
//! The phone connects, pins that certificate, and trades the code for a
//! 256-bit device token. Only SHA-256 hashes of codes and tokens are stored.

use std::time::Duration;

use idb_store::Journal;
use sha2::{Digest, Sha256};

/// Pairing codes expire after this long.
pub const CODE_TTL: Duration = Duration::from_secs(5 * 60);
/// URI scheme and version understood by the mobile app.
pub const URI_PREFIX: &str = "idb://pair";
pub const URI_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PairingError {
    #[error("pairing code unknown, used or expired")]
    InvalidCode,
    #[error("device token unknown or revoked")]
    Unauthorized,
    #[error("malformed pairing URI")]
    BadUri,
    #[error("no secure random source available")]
    Entropy,
    #[error("journal: {0}")]
    Store(String),
}

impl From<idb_store::StoreError> for PairingError {
    fn from(e: idb_store::StoreError) -> Self {
        PairingError::Store(e.to_string())
    }
}

/// What the phone needs to reach and trust this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingOffer {
    pub host: String,
    pub port: u16,
    /// Hex SHA-256 of the listener's TLS certificate (pinned by the phone).
    pub cert_sha256: String,
    /// Single-use code (hex, 128 bits). Shown once; never stored in clear.
    pub code: String,
    pub expires_at: u64,
}

impl PairingOffer {
    pub fn uri(&self) -> String {
        format!(
            "{URI_PREFIX}?v={URI_VERSION}&h={}&p={}&fp={}&c={}",
            self.host, self.port, self.cert_sha256, self.code
        )
    }

    /// Parses an offer URI (used by the phone side and by tests).
    pub fn parse(uri: &str) -> Result<PairingOffer, PairingError> {
        let query = uri
            .strip_prefix(URI_PREFIX)
            .and_then(|r| r.strip_prefix('?'))
            .ok_or(PairingError::BadUri)?;
        let get = |k: &str| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix(k).and_then(|v| v.strip_prefix('=')))
                .map(str::to_owned)
        };
        if get("v").and_then(|v| v.parse::<u32>().ok()) != Some(URI_VERSION) {
            return Err(PairingError::BadUri);
        }
        let host = get("h")
            .filter(|h| !h.is_empty())
            .ok_or(PairingError::BadUri)?;
        let port = get("p")
            .and_then(|p| p.parse().ok())
            .ok_or(PairingError::BadUri)?;
        let cert_sha256 = get("fp")
            .filter(|f| f.len() == 64 && f.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or(PairingError::BadUri)?;
        let code = get("c")
            .filter(|c| c.len() == 32 && c.chars().all(|c| c.is_ascii_hexdigit()))
            .ok_or(PairingError::BadUri)?;
        Ok(PairingOffer {
            host,
            port,
            cert_sha256,
            code,
            expires_at: 0,
        })
    }
}

fn random_hex(bytes: usize) -> Result<String, PairingError> {
    let mut buf = vec![0u8; bytes];
    getrandom::fill(&mut buf).map_err(|_| PairingError::Entropy)?;
    Ok(hex(&buf))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Hex SHA-256; what the journal stores instead of secrets.
pub fn hash_secret(secret: &str) -> String {
    hex(&Sha256::digest(secret.as_bytes()))
}

/// Constant-time comparison of two equal-length strings.
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Creates a new single-use pairing offer and records its code's hash.
pub fn new_offer(
    journal: &Journal,
    host: &str,
    port: u16,
    cert_sha256: &str,
    now: u64,
) -> Result<PairingOffer, PairingError> {
    let code = random_hex(16)?;
    let expires_at = now + CODE_TTL.as_secs();
    journal.add_pairing_code(&hash_secret(&code), expires_at)?;
    Ok(PairingOffer {
        host: host.to_owned(),
        port,
        cert_sha256: cert_sha256.to_owned(),
        code,
        expires_at,
    })
}

/// Trades a valid code for a new device token (returned once, in clear).
pub fn redeem(
    journal: &Journal,
    code: &str,
    device_name: &str,
    now: u64,
) -> Result<(u64, String), PairingError> {
    if !journal.consume_pairing_code(&hash_secret(code), now)? {
        return Err(PairingError::InvalidCode);
    }
    let token = random_hex(32)?;
    let name: String = device_name
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    let id = journal.add_device(&name, &hash_secret(&token), now)?;
    Ok((id, token))
}

/// Checks a device token; returns the device id.
pub fn authenticate(journal: &Journal, token: &str, now: u64) -> Result<u64, PairingError> {
    // Lookup is by hash, so timing does not reveal the token.
    journal
        .touch_device(&hash_secret(token), now)?
        .ok_or(PairingError::Unauthorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    #[test]
    fn full_flow_single_use_expiry_and_revocation() {
        let j = Journal::open_in_memory().unwrap();
        let offer = new_offer(&j, "192.168.0.10", 7443, FP, 1000).unwrap();
        let parsed = PairingOffer::parse(&offer.uri()).unwrap();
        assert_eq!((parsed.host.as_str(), parsed.port), ("192.168.0.10", 7443));
        assert_eq!(parsed.code, offer.code);

        let (id, token) = redeem(&j, &offer.code, "Pixel da Ana", 1010).unwrap();
        assert_eq!(token.len(), 64);
        // Single use.
        assert_eq!(
            redeem(&j, &offer.code, "x", 1011),
            Err(PairingError::InvalidCode)
        );
        assert_eq!(authenticate(&j, &token, 1020), Ok(id));
        assert_eq!(
            authenticate(&j, "nope", 1020),
            Err(PairingError::Unauthorized)
        );
        let devices = j.devices().unwrap();
        assert_eq!(devices[0].name, "Pixel da Ana");
        assert_eq!(devices[0].last_seen, Some(1020));

        assert!(j.revoke_device(id).unwrap());
        assert_eq!(
            authenticate(&j, &token, 1030),
            Err(PairingError::Unauthorized)
        );

        // Expired code.
        let late = new_offer(&j, "h", 1, FP, 2000).unwrap();
        let too_late = 2000 + CODE_TTL.as_secs() + 1;
        assert_eq!(
            redeem(&j, &late.code, "x", too_late),
            Err(PairingError::InvalidCode)
        );
    }

    #[test]
    fn secrets_are_never_stored_in_clear() {
        let j = Journal::open_in_memory().unwrap();
        let offer = new_offer(&j, "h", 1, FP, 0).unwrap();
        let (_, token) = redeem(&j, &offer.code, "p", 1).unwrap();
        assert_ne!(hash_secret(&token), token);
        assert!(!j.devices().unwrap().is_empty());
    }

    #[test]
    fn rejects_malformed_uris() {
        for bad in [
            "https://x",
            "idb://pair?v=2&h=a&p=1&fp=00&c=00",
            &format!("idb://pair?v=1&h=a&p=x&fp={FP}&c={}", "0".repeat(32)),
            &format!("idb://pair?v=1&h=a&p=1&fp=short&c={}", "0".repeat(32)),
        ] {
            assert_eq!(PairingOffer::parse(bad), Err(PairingError::BadUri), "{bad}");
        }
    }

    #[test]
    fn constant_time_eq() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "ab"));
    }
}
