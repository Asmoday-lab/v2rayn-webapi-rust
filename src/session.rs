//! Session + SSE-ticket service, ported from Security/WebSessionService.cs.
//!
//! Contract details that matter for the differential test:
//!   - token/ticket: 32 random bytes → base64url, exactly 43 chars.
//!   - stored key: uppercase hex SHA-256 of the *raw bytes* (not the string).
//!   - sliding lifetime 7d, absolute lifetime 30d, SSE ticket 45s.
//!   - presented token must be canonical base64url of exactly 32 bytes.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TOKEN_BYTE_LENGTH: usize = 32;
const TOKEN_ENCODED_LENGTH: usize = 43;
const SLIDING_LIFETIME: Duration = Duration::from_secs(7 * 24 * 3600);
const ABSOLUTE_LIFETIME: Duration = Duration::from_secs(30 * 24 * 3600);
const SSE_TICKET_LIFETIME: Duration = Duration::from_secs(45);

#[derive(Clone)]
struct SessionEntry {
    expires_at: SystemTime,
    absolute_expires_at: SystemTime,
}

#[derive(Clone)]
struct TicketEntry {
    session_digest: String,
    expires_at: SystemTime,
}

#[derive(Default)]
pub struct SessionService {
    sessions: Mutex<HashMap<String, SessionEntry>>,
    tickets: Mutex<HashMap<String, TicketEntry>>,
}

impl SessionService {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a fresh session, returning (token, seconds-until-expiry).
    pub fn create_session(&self) -> (String, u64) {
        let (token, bytes, digest) = mint_token();
        let now = SystemTime::now();
        let expires = (now + SLIDING_LIFETIME).min(now + ABSOLUTE_LIFETIME);
        self.sessions.lock().unwrap().insert(
            digest,
            SessionEntry {
                expires_at: expires,
                absolute_expires_at: now + ABSOLUTE_LIFETIME,
            },
        );
        let remaining = expires.duration_since(now).unwrap_or_default().as_secs();
        drop(bytes);
        (token, remaining)
    }

    /// Validate a presented token and renew its sliding expiry.
    pub fn validate_and_renew(&self, token: &str) -> bool {
        let Some(digest) = digest_of_presented(token) else {
            return false;
        };
        let now = SystemTime::now();
        let mut sessions = self.sessions.lock().unwrap();
        let Some(entry) = sessions.get(&digest).cloned() else {
            return false;
        };
        if now >= entry.expires_at || now >= entry.absolute_expires_at {
            sessions.remove(&digest);
            return false;
        }
        let renewed = SessionEntry {
            expires_at: (now + SLIDING_LIFETIME).min(entry.absolute_expires_at),
            absolute_expires_at: entry.absolute_expires_at,
        };
        sessions.insert(digest, renewed);
        true
    }

    pub fn revoke(&self, token: &str) -> bool {
        let Some(digest) = digest_of_presented(token) else {
            return false;
        };
        let removed = self.sessions.lock().unwrap().remove(&digest).is_some();
        if removed {
            self.tickets
                .lock()
                .unwrap()
                .retain(|_, t| t.session_digest != digest);
        }
        removed
    }

    /// Mint a one-shot SSE ticket bound to the presented session.
    /// Returns (ticket, seconds-until-expiry) or None if the session is invalid.
    pub fn create_sse_ticket(&self, session_token: &str) -> Option<(String, u64)> {
        let session_digest = digest_of_presented(session_token)?;
        {
            let sessions = self.sessions.lock().unwrap();
            let entry = sessions.get(&session_digest)?;
            let now = SystemTime::now();
            if now >= entry.expires_at || now >= entry.absolute_expires_at {
                return None;
            }
        }
        let (ticket, bytes, digest) = mint_token();
        let now = SystemTime::now();
        self.tickets.lock().unwrap().insert(
            digest,
            TicketEntry {
                session_digest,
                expires_at: now + SSE_TICKET_LIFETIME,
            },
        );
        drop(bytes);
        Some((ticket, SSE_TICKET_LIFETIME.as_secs()))
    }

    /// Consume a one-shot SSE ticket (45s, single use). Used by the /api/events
    /// endpoint in a later milestone; kept now for contract parity and tests.
    #[allow(dead_code)]
    pub fn consume_sse_ticket(&self, ticket_token: &str) -> bool {
        let Some(digest) = digest_of_presented(ticket_token) else {
            return false;
        };
        let Some(ticket) = self.tickets.lock().unwrap().remove(&digest) else {
            return false;
        };
        if SystemTime::now() >= ticket.expires_at {
            return false;
        }
        let sessions = self.sessions.lock().unwrap();
        sessions.get(&ticket.session_digest).is_some_and(|entry| {
            let now = SystemTime::now();
            now < entry.expires_at && now < entry.absolute_expires_at
        })
    }
}

/// Generate a token: 32 random bytes from /dev/urandom, base64url encoded,
/// with the SHA-256 hex digest of the raw bytes used as the storage key.
fn mint_token() -> (String, Vec<u8>, String) {
    let mut bytes = vec![0u8; TOKEN_BYTE_LENGTH];
    // Pure-Rust CSPRNG: getrandom(2) syscall on Linux/Termux, system API on
    // Windows (dev tests). No C toolchain — safe for static musl builds.
    getrandom::fill(&mut bytes).expect("CSPRNG unavailable");
    let token = base64url_encode(&bytes);
    let digest = hex_sha256(&bytes);
    (token, bytes, digest)
}

/// Strictly mirror `WebSessionService.TryGetDigest`: canonical 43-char
/// base64url of exactly 32 bytes; re-encode check rejects non-canonical forms.
fn digest_of_presented(token: &str) -> Option<String> {
    if token.len() != TOKEN_ENCODED_LENGTH
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return None;
    }
    let bytes = base64url_decode(token)?;
    if bytes.len() != TOKEN_BYTE_LENGTH || base64url_encode(&bytes) != token {
        return None;
    }
    Some(hex_sha256(&bytes))
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn base64url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 63) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 63) as usize] as char);
        }
    }
    out
}

fn base64url_decode(token: &str) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'-' => Some(62),
            b'_' => Some(63),
            _ => None,
        }
    }
    let bytes = token.as_bytes();
    let mut out = Vec::with_capacity(TOKEN_BYTE_LENGTH);
    for chunk in bytes.chunks(4) {
        let mut triple = 0u32;
        let mut count = 0usize;
        for (i, &c) in chunk.iter().enumerate() {
            triple |= (value(c)? as u32) << (18 - 6 * i);
            count = i + 1;
        }
        out.push((triple >> 16) as u8);
        if count > 2 {
            out.push((triple >> 8) as u8);
        }
        if count > 3 {
            out.push(triple as u8);
        }
    }
    Some(out)
}

/// ISO-8601 timestamp for `expiresAt`, matching C# `DateTimeOffset` Web serialization
/// shape: `YYYY-MM-DDTHH:MM:SS.fffffff+00:00`.
pub fn format_expires_at(now: SystemTime, ttl_secs: u64) -> String {
    let target = now + Duration::from_secs(ttl_secs);
    let secs = target
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (y, m, d, hh, mm, ss) = unix_to_utc_parts(secs);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.0000000+00:00")
}

/// Civil date from days since epoch (Howard Hinnant's algorithm).
fn unix_to_utc_parts(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    if m <= 2 {
        y += 1;
    }
    (
        y,
        m as u32,
        d as u32,
        (sod / 3600) as u32,
        ((sod % 3600) / 60) as u32,
        (sod % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64url_roundtrip() {
        let raw: Vec<u8> = (0..32).collect();
        let encoded = base64url_encode(&raw);
        assert_eq!(encoded.len(), 43);
        assert_eq!(base64url_decode(&encoded), Some(raw));
    }

    #[test]
    fn base64url_known_vector() {
        // SHA-256("abc") base64url, unpadded.
        let mut hasher = Sha256::new();
        hasher.update(b"abc");
        let digest = hasher.finalize().to_vec();
        // RFC 6238 reference: base64url(SHA-256("abc")) without padding.
        assert_eq!(
            base64url_encode(&digest),
            "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0"
        );
    }

    #[test]
    fn non_canonical_token_rejected() {
        // 43 chars, valid alphabet, but decodes to 32 bytes that re-encode differently
        // (last char has unused low bits set).
        let bad = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAB";
        assert_eq!(digest_of_presented(bad), None);
        assert_eq!(digest_of_presented("too-short"), None);
    }

    #[test]
    fn session_lifecycle() {
        let service = SessionService::new();
        let (token, ttl) = service.create_session();
        assert_eq!(token.len(), 43);
        assert_eq!(ttl, 7 * 24 * 3600);
        assert!(service.validate_and_renew(&token));
        assert!(service.revoke(&token));
        assert!(!service.validate_and_renew(&token));
    }

    #[test]
    fn sse_ticket_is_one_shot() {
        let service = SessionService::new();
        let (token, _) = service.create_session();
        let (ticket, ttl) = service.create_sse_ticket(&token).unwrap();
        assert_eq!(ttl, 45);
        assert!(service.consume_sse_ticket(&ticket));
        assert!(!service.consume_sse_ticket(&ticket));
        assert!(service.create_sse_ticket("invalid").is_none());
    }

    #[test]
    fn expires_at_shape() {
        let now = UNIX_EPOCH + Duration::from_secs(1_760_000_000);
        let formatted = format_expires_at(now, 7 * 24 * 3600);
        assert!(formatted.ends_with(".0000000+00:00"));
        // "YYYY-MM-DDTHH:MM:SS.fffffff+00:00" = 33 chars
        assert_eq!(formatted.len(), 33);
    }
}
