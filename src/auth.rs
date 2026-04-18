use crate::platform;
use anyhow::{Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

// ── PKCE (RFC 7636) ────────────────────────────────────────────────────

pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub method: String,
}

/// Generate a PKCE challenge pair (RFC 7636).
pub fn generate_pkce() -> Result<PkceChallenge> {
    let mut random_bytes = [0u8; 64];
    OsRng.fill_bytes(&mut random_bytes);

    let verifier = URL_SAFE_NO_PAD.encode(random_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();

    let challenge = URL_SAFE_NO_PAD.encode(hash);

    Ok(PkceChallenge {
        verifier,
        challenge,
        method: "S256".to_string(),
    })
}

// ── OAuth Token ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    #[serde(default = "default_token_type")]
    pub token_type: String,
}

fn default_token_type() -> String {
    "Bearer".to_string()
}

impl OAuthToken {
    /// Returns true if the token is expired or within 300s of expiring.
    pub fn is_expired(&self) -> bool {
        if self.expires_at == 0 {
            return false;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now + 300 >= self.expires_at
    }
}

// ── Credential Store ───────────────────────────────────────────────────

const CRED_DIR: &str = ".openpaw";
const CRED_FILE: &str = "auth.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: i64,
    pub token_type: String,
}

impl From<OAuthToken> for StoredToken {
    fn from(token: OAuthToken) -> Self {
        Self {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: token.expires_at,
            token_type: token.token_type,
        }
    }
}

impl From<StoredToken> for OAuthToken {
    fn from(token: StoredToken) -> Self {
        Self {
            access_token: token.access_token,
            refresh_token: token.refresh_token,
            expires_at: token.expires_at,
            token_type: token.token_type,
        }
    }
}

/// Set owner-only read/write permissions on a file (Unix: 0600).
/// On non-Unix platforms this is a no-op; tighten as needed per platform.
fn set_secure_permissions(file: &fs::File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = file.metadata()?.permissions();
        perms.set_mode(0o600);
        file.set_permissions(perms)?;
    }
    let _ = file; // suppress unused warning on non-Unix
    Ok(())
}

pub fn save_credential(provider: &str, token: OAuthToken) -> Result<()> {
    let home = platform::get_home_dir().context("Home directory not found")?;
    let dir_path = home.join(CRED_DIR);
    fs::create_dir_all(&dir_path)?;

    let file_path = dir_path.join(CRED_FILE);

    let mut existing: HashMap<String, StoredToken> = if file_path.exists() {
        let file = fs::File::open(&file_path)?;
        serde_json::from_reader(file).unwrap_or_default()
    } else {
        HashMap::new()
    };

    existing.insert(provider.to_string(), token.into());

    let file = fs::File::create(&file_path)?;
    set_secure_permissions(&file)?;
    serde_json::to_writer(file, &existing)?;
    Ok(())
}

pub fn load_credential(provider: &str) -> Result<Option<OAuthToken>> {
    let home = platform::get_home_dir().context("Home directory not found")?;
    let file_path = home.join(CRED_DIR).join(CRED_FILE);

    if !file_path.exists() {
        return Ok(None);
    }

    let file = fs::File::open(&file_path)?;
    let existing: HashMap<String, StoredToken> = serde_json::from_reader(file)?;

    if let Some(stored) = existing.get(provider) {
        let token: OAuthToken = stored.clone().into();
        if token.is_expired() {
            return Ok(None);
        }
        Ok(Some(token))
    } else {
        Ok(None)
    }
}

pub fn delete_credential(provider: &str) -> Result<bool> {
    let home = platform::get_home_dir().context("Home directory not found")?;
    let file_path = home.join(CRED_DIR).join(CRED_FILE);

    if !file_path.exists() {
        return Ok(false);
    }

    let file = fs::File::open(&file_path)?;
    let mut existing: HashMap<String, StoredToken> = serde_json::from_reader(file)?;

    if existing.remove(provider).is_some() {
        let file = fs::File::create(&file_path)?;
        set_secure_permissions(&file)?;
        serde_json::to_writer(file, &existing)?;
        Ok(true)
    } else {
        Ok(false)
    }
}

// ── Token Refresh ─────────────────────────────────────────────────────

pub fn refresh_access_token(
    token_url: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<OAuthToken> {
    let client = Client::new();
    let body_str = format!(
        "grant_type=refresh_token&refresh_token={}&client_id={}",
        urlencoding::encode(refresh_token),
        urlencoding::encode(client_id)
    );

    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", "openpaw/1.0")
        .body(body_str)
        .send()?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!("Token refresh failed: {}", resp.status()));
    }

    let mut token: OAuthToken = resp.json()?;

    // Preserve old refresh_token if response omits a new one
    if token.refresh_token.is_none() {
        token.refresh_token = Some(refresh_token.to_string());
    }

    Ok(token)
}

// ── Device Code Flow (RFC 8628) ────────────────────────────────────────

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: u32,
    pub expires_in: u32,
}

pub fn start_device_code_flow(
    client_id: &str,
    device_auth_url: &str,
    scope: &str,
) -> Result<DeviceCode> {
    let client = Client::new();
    let body_str = format!(
        "client_id={}&scope={}",
        urlencoding::encode(client_id),
        urlencoding::encode(scope)
    );

    let resp = client
        .post(device_auth_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("User-Agent", "openpaw/1.0")
        .body(body_str)
        .send()?;

    if !resp.status().is_success() {
        return Err(anyhow::anyhow!(
            "Device code request failed: {}",
            resp.status()
        ));
    }

    let device_code: DeviceCode = resp.json()?;
    Ok(device_code)
}
