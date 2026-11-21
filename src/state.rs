//! Shared application state: management-key verification, allowed origins,
//! rate limiter. Ported from Security/WebAuthService.cs and
//! Security/WebAuthRateLimiting.cs.

use hmac::Hmac;
use pbkdf2::pbkdf2;
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

pub const MINIMUM_KEY_LENGTH: usize = 12;
pub const MAXIMUM_KEY_LENGTH: usize = 4096;
const PBKDF2_ITERATIONS: u32 = 600_000;

/// Fixed-window rate limiter, one window per partition key.
/// C#: permitLimit 5 (login) / 10 (setup), 1-minute window, auto replenishment.
#[derive(Default)]
pub struct RateLimiter {
    windows: Mutex<HashMap<(String, String), (Instant, u32)>>,
}

impl RateLimiter {
    pub fn allow(&self, partition: &str, policy: &str) -> bool {
        let limit = if policy == "/api/auth/login" { 5 } else { 10 };
        let now = Instant::now();
        let mut windows = self.windows.lock().unwrap();
        let key = (partition.to_string(), policy.to_string());
        match windows.get_mut(&key) {
            Some((start, count)) if now.duration_since(*start).as_secs() < 60 => {
                if *count < limit {
                    *count += 1;
                    true
                } else {
                    false
                }
            }
            _ => {
                windows.insert(key, (now, 1));
                true
            }
        }
    }
}

/// Verify a management key exactly like `WebAuthService.ValidateManagementKey`.
/// In CI/Termux the key comes from `V2RAYN_WEB_API_KEY` (environment path):
/// a fixed-time byte comparison, empty input rejected. The stored-hash path
/// (PBKDF2 config file) is retained for parity with the desktop deployment.
#[derive(Clone)]
pub struct AuthService {
    environment_key: Option<String>,
    stored: Option<StoredAuth>,
    dummy_salt: [u8; 16],
}

#[derive(Clone)]
struct StoredAuth {
    iterations: u32,
    salt: Vec<u8>,
    verifier: Vec<u8>,
}

impl AuthService {
    pub fn from_environment(key: Option<String>) -> Self {
        let normalized = key.filter(|k| !k.trim().is_empty());
        Self {
            environment_key: normalized,
            stored: None,
            dummy_salt: [0x5a; 16],
        }
    }

    pub fn environment_key_configured(&self) -> bool {
        self.environment_key.is_some()
    }

    pub fn setup_required(&self) -> bool {
        self.environment_key.is_none() && self.stored.is_none()
    }

    pub fn validate(&self, supplied: &str) -> bool {
        if supplied.is_empty() || supplied.len() > MAXIMUM_KEY_LENGTH {
            // Match C#'s dummy work on the environment-key path (timing parity).
            let _ = derive_verifier(supplied, &self.dummy_salt, PBKDF2_ITERATIONS);
            return false;
        }
        if let Some(expected) = &self.environment_key {
            return constant_time_eq_str(expected, supplied);
        }
        match &self.stored {
            None => false,
            Some(stored) => {
                let supplied_hash = derive_verifier(supplied, &stored.salt, stored.iterations);
                constant_time_eq_bytes(&supplied_hash, &stored.verifier)
            }
        }
    }
}

fn derive_verifier(key: &str, salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut output = vec![0u8; 32];
    let _ = pbkdf2::<Hmac<Sha256>>(key.as_bytes(), salt, iterations, &mut output);
    output
}

fn constant_time_eq_str(expected: &str, actual: &str) -> bool {
    constant_time_eq_bytes(expected.as_bytes(), actual.as_bytes())
}

fn constant_time_eq_bytes(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[derive(Clone)]
pub struct AppState {
    pub auth: AuthService,
    pub sessions: std::sync::Arc<crate::session::SessionService>,
    pub allowed_origins: Vec<String>,
    pub rate_limiter: std::sync::Arc<RateLimiter>,
    pub instance_pid: u32,
    pub store: std::sync::Arc<crate::store::Store>,
    pub core: std::sync::Arc<crate::core::CoreHandle>,
}

impl AppState {
    pub fn from_environment() -> Self {
        let allowed_origins = std::env::var("V2RAYN_WEB_ALLOWED_ORIGINS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| {
                s.split(',')
                    .filter_map(|item| {
                        let trimmed = item.trim();
                        // Mirror WebCorsPolicy: only well-formed http(s) origins.
                        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                            let authority = trimmed
                                .trim_start_matches("https://")
                                .trim_start_matches("http://")
                                .trim_end_matches('/');
                            if !authority.is_empty()
                                && !authority.contains(['*', '\\', '?', '#', '@'])
                            {
                                Some(trimmed.trim_end_matches('/').to_string())
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        Self {
            auth: AuthService::from_environment(std::env::var("V2RAYN_WEB_API_KEY").ok()),
            sessions: std::sync::Arc::new(crate::session::SessionService::new()),
            allowed_origins,
            rate_limiter: std::sync::Arc::new(RateLimiter::default()),
            instance_pid: std::process::id(),
            store: std::sync::Arc::new(crate::store::Store::open(
                &crate::store::data_home_from_env(),
            )),
            core: std::sync::Arc::new(crate::core::CoreHandle::new()),
        }
    }

    /// Test-only state with an explicit allowlist and no management key.
    #[cfg(test)]
    pub fn for_tests(allowed_origins: &[String]) -> Self {
        Self {
            auth: AuthService::from_environment(None),
            sessions: std::sync::Arc::new(crate::session::SessionService::new()),
            allowed_origins: allowed_origins.to_vec(),
            rate_limiter: std::sync::Arc::new(RateLimiter::default()),
            instance_pid: 0,
            store: std::sync::Arc::new(crate::store::Store::open(
                &std::env::temp_dir().join(format!("nova-state-tests-{}", std::process::id())),
            )),
            core: std::sync::Arc::new(crate::core::CoreHandle::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_env_key_validation() {
        let auth = AuthService::from_environment(Some("ci-oracle-key-1234567890".to_string()));
        assert!(auth.environment_key_configured());
        assert!(!auth.setup_required());
        assert!(auth.validate("ci-oracle-key-1234567890"));
        assert!(!auth.validate("wrong"));
        assert!(!auth.validate(""));
    }

    #[test]
    fn auth_no_key_requires_setup() {
        let auth = AuthService::from_environment(None);
        assert!(!auth.environment_key_configured());
        assert!(auth.setup_required());
        assert!(!auth.validate("anything"));
    }

    #[test]
    fn rate_limiter_enforces_per_policy_limit() {
        let limiter = RateLimiter::default();
        for _ in 0..5 {
            assert!(limiter.allow("loopback", "/api/auth/login"));
        }
        assert!(!limiter.allow("loopback", "/api/auth/login"));
        // Setup policy has a separate budget.
        for _ in 0..10 {
            assert!(limiter.allow("loopback", "/api/setup"));
        }
        assert!(!limiter.allow("loopback", "/api/setup"));
    }
}
