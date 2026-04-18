//! Secret encryption using ChaCha20-Poly1305 AEAD.
//!
//! Secrets (API keys, tokens, etc.) are encrypted using ChaCha20-Poly1305
//! with a random key stored on disk with restrictive file permissions.
//!
//! Encrypted secrets are prefixed with "enc2:" to distinguish them from plaintext.

use anyhow::{anyhow, Context, Result};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    ChaCha20Poly1305, Nonce,
};
use rand::RngCore;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// Length of the encryption key in bytes (256-bit).
pub const KEY_LEN: usize = 32;

/// ChaCha20-Poly1305 nonce length in bytes.
pub const NONCE_LEN: usize = 12;

/// Tag length (authentication code).
pub const TAG_LEN: usize = 16;

/// Prefix for encrypted secrets.
pub const ENCRYPTED_PREFIX: &str = "enc2:";

/// Manages encrypted storage of secrets (API keys, tokens, etc.)
#[derive(Debug, Clone)]
pub struct SecretStore {
    /// Path to the key file
    key_path: PathBuf,
    /// Whether encryption is enabled
    enabled: bool,
}

impl SecretStore {
    /// Create a new secret store rooted at the given directory.
    pub fn new(dir: &Path, enabled: bool) -> Self {
        let key_path = dir.join(".secret_key");
        Self { key_path, enabled }
    }

    /// Check if a value is encrypted (has "enc2:" prefix).
    pub fn is_encrypted(value: &str) -> bool {
        value.starts_with(ENCRYPTED_PREFIX)
    }

    /// Encrypt a plaintext secret. Returns hex-encoded ciphertext prefixed with "enc2:".
    /// Format: enc2:<hex(nonce || ciphertext || tag)> (12 + N + 16 bytes).
    /// If encryption is disabled or plaintext is empty, returns the plaintext as-is.
    pub fn encrypt(&self, plaintext: &str) -> Result<String> {
        if !self.enabled || plaintext.is_empty() {
            return Ok(plaintext.to_string());
        }

        let key = self.load_or_create_key()?;
        let cipher = ChaCha20Poly1305::new_from_slice(key.as_ref())
            .map_err(|e| anyhow!("Failed to initialize cipher: {}", e))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| anyhow!("Encryption failed: {}", e))?;

        // Build blob: nonce || ciphertext
        let mut blob = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ciphertext);

        // Hex encode and prepend "enc2:"
        let hex_encoded = hex::encode(&blob);
        Ok(format!("{}{}", ENCRYPTED_PREFIX, hex_encoded))
    }

    /// Decrypt a secret.
    /// - "enc2:" prefix -> ChaCha20-Poly1305 decryption
    /// - No prefix -> returned as-is (plaintext)
    pub fn decrypt(&self, value: &str) -> Result<String> {
        if !value.starts_with(ENCRYPTED_PREFIX) {
            return Ok(value.to_string());
        }

        let hex_str = &value[ENCRYPTED_PREFIX.len()..];

        // Decode hex
        let blob =
            hex::decode(hex_str).map_err(|e| anyhow!("Corrupt hex in encrypted secret: {}", e))?;

        if blob.len() <= NONCE_LEN {
            return Err(anyhow!("Ciphertext too short"));
        }

        let nonce_bytes: [u8; NONCE_LEN] = blob[0..NONCE_LEN]
            .try_into()
            .map_err(|_| anyhow!("Invalid nonce length"))?;
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = &blob[NONCE_LEN..];

        let key = self.load_or_create_key()?;
        let cipher = ChaCha20Poly1305::new_from_slice(key.as_ref())
            .map_err(|e| anyhow!("Failed to initialize cipher: {}", e))?;

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|_| anyhow!("Decryption failed - key may have changed or data corrupted"))?;

        String::from_utf8(plaintext)
            .map_err(|e| anyhow!("Invalid UTF-8 in decrypted secret: {}", e))
    }
}

impl SecretStore {
    /// Load the encryption key from disk, or create one if it doesn't exist.
    fn load_or_create_key(&self) -> Result<Zeroizing<[u8; KEY_LEN]>> {
        // Try to read existing key
        if self.key_path.exists() {
            let mut file = File::open(&self.key_path)
                .with_context(|| format!("Failed to open key file: {:?}", self.key_path))?;

            let mut hex_str = String::new();
            file.read_to_string(&mut hex_str)
                .with_context(|| "Failed to read key file")?;

            let hex_str = hex_str.trim();
            let key_bytes =
                hex::decode(hex_str).with_context(|| "Key file contains invalid hex")?;

            if key_bytes.len() != KEY_LEN {
                return Err(anyhow!(
                    "Key file has wrong length: expected {} bytes, got {}",
                    KEY_LEN,
                    key_bytes.len()
                ));
            }

            let mut key = Zeroizing::new([0u8; KEY_LEN]);
            key.copy_from_slice(&key_bytes);
            return Ok(key);
        }

        // Generate new key
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        OsRng.fill_bytes(key.as_mut());

        // Write hex-encoded key
        let hex_encoded = hex::encode(key.as_ref());

        // Ensure parent dir exists
        if let Some(parent) = self.key_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create key directory: {:?}", parent))?;
        }

        // Write key file with restrictive permissions
        let mut file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.key_path)
            .with_context(|| format!("Failed to create key file: {:?}", self.key_path))?;

        file.write_all(hex_encoded.as_bytes())
            .with_context(|| "Failed to write key file")?;

        // Set restrictive permissions (Unix: 0600, owner-only)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = file
                .metadata()
                .with_context(|| "Failed to get key file metadata")?;
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o600);
            fs::set_permissions(&self.key_path, permissions)
                .with_context(|| "Failed to set key file permissions")?;
        }

        // On Windows, use icacls to strip inherited ACEs and grant only the
        // current user full control, preventing other local users from reading
        // the master encryption key.
        #[cfg(windows)]
        {
            // Obtain current username via USERPROFILE or USERNAME env var.
            let username = std::env::var("USERNAME").unwrap_or_default();
            if !username.is_empty() {
                let path_str = self.key_path.to_string_lossy();
                // /inheritance:r  – remove inherited permissions
                // /grant:r        – replace (not add) a grant ACE
                let _ = std::process::Command::new("icacls")
                    .arg(path_str.as_ref())
                    .arg("/inheritance:r")
                    .arg("/grant:r")
                    .arg(format!("{}:F", username))
                    .output(); // best-effort; proceed even if icacls is unavailable
            }
        }

        Ok(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_temp_store() -> (TempDir, SecretStore) {
        let temp_dir = TempDir::new().unwrap();
        let store = SecretStore::new(temp_dir.path(), true);
        (temp_dir, store)
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let (_temp_dir, store) = create_temp_store();
        let secret = "sk-my-secret-api-key-12345";

        let encrypted = store.encrypt(secret).unwrap();
        assert!(encrypted.starts_with("enc2:"));
        assert_ne!(encrypted, secret);

        let decrypted = store.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, secret);
    }

    #[test]
    fn test_encrypt_empty_returns_empty() {
        let (_temp_dir, store) = create_temp_store();
        let result = store.encrypt("").unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn test_decrypt_plaintext_passthrough() {
        let (_temp_dir, store) = create_temp_store();
        let result = store.decrypt("sk-plaintext-key").unwrap();
        assert_eq!(result, "sk-plaintext-key");
    }

    #[test]
    fn test_disabled_returns_plaintext() {
        let temp_dir = TempDir::new().unwrap();
        let store = SecretStore::new(temp_dir.path(), false);
        let result = store.encrypt("sk-secret").unwrap();
        assert_eq!(result, "sk-secret");
    }

    #[test]
    fn test_is_encrypted() {
        assert!(SecretStore::is_encrypted("enc2:aabbcc"));
        assert!(!SecretStore::is_encrypted("sk-plaintext"));
        assert!(!SecretStore::is_encrypted(""));
        assert!(!SecretStore::is_encrypted("enc"));
        assert!(!SecretStore::is_encrypted("enc2"));
        assert!(SecretStore::is_encrypted("enc2:x"));
    }

    #[test]
    fn test_encrypting_same_value_produces_different_ciphertext() {
        let (_temp_dir, store) = create_temp_store();
        let e1 = store.encrypt("secret").unwrap();
        let e2 = store.encrypt("secret").unwrap();

        assert_ne!(e1, e2);

        // Both should decrypt to same value
        let d1 = store.decrypt(&e1).unwrap();
        let d2 = store.decrypt(&e2).unwrap();
        assert_eq!(d1, "secret");
        assert_eq!(d2, "secret");
    }

    #[test]
    fn test_different_dirs_cannot_decrypt_each_other() {
        let temp1 = TempDir::new().unwrap();
        let store1 = SecretStore::new(temp1.path(), true);

        let temp2 = TempDir::new().unwrap();
        let store2 = SecretStore::new(temp2.path(), true);

        let encrypted = store1.encrypt("secret-for-store1").unwrap();

        // store2 should not be able to decrypt store1's secret
        let result = store2.decrypt(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_same_dir_interop() {
        let temp_dir = TempDir::new().unwrap();
        let store1 = SecretStore::new(temp_dir.path(), true);
        let store2 = SecretStore::new(temp_dir.path(), true);

        let encrypted = store1.encrypt("cross-store-secret").unwrap();
        let decrypted = store2.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, "cross-store-secret");
    }

    #[test]
    fn test_unicode_roundtrip() {
        let (_temp_dir, store) = create_temp_store();
        let secret = "sk-émojis-🦀-test";

        let encrypted = store.encrypt(secret).unwrap();
        let decrypted = store.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, secret);
    }

    #[test]
    fn test_key_file_created_on_first_encrypt() {
        let (temp_dir, store) = create_temp_store();
        let key_path = temp_dir.path().join(".secret_key");

        // Key file should not exist yet
        assert!(!key_path.exists());

        // Encrypt should create key file
        let _ = store.encrypt("trigger-key-creation").unwrap();
        assert!(key_path.exists());

        // Key file should contain 64 hex chars (32 bytes)
        let content = fs::read_to_string(&key_path).unwrap();
        assert_eq!(content.trim().len(), KEY_LEN * 2);
    }

    #[test]
    fn test_tampered_ciphertext_detected() {
        let (_temp_dir, store) = create_temp_store();
        let encrypted = store.encrypt("sensitive-data").unwrap();

        // Tamper with the hex string (flip a character)
        let mut tampered = encrypted.clone();
        let bytes = unsafe { tampered.as_bytes_mut() };
        if bytes.len() > 6 {
            bytes[6] = if bytes[6] == b'0' { b'1' } else { b'0' };
        }

        let result = store.decrypt(&tampered);
        assert!(result.is_err());
    }

    #[test]
    fn test_corrupt_hex_returns_error() {
        let (_temp_dir, store) = create_temp_store();
        // Trigger key creation
        let _ = store.encrypt("setup").unwrap();

        let result = store.decrypt("enc2:not-valid-hex!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_truncated_ciphertext_returns_error() {
        let (_temp_dir, store) = create_temp_store();
        // Trigger key creation
        let _ = store.encrypt("setup").unwrap();

        let result = store.decrypt("enc2:aabb");
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_values_same_store() {
        let (_temp_dir, store) = create_temp_store();
        let secrets = vec!["secret-1", "secret-2", "secret-3"];
        let mut encrypted = Vec::new();

        for secret in &secrets {
            encrypted.push(store.encrypt(secret).unwrap());
        }

        for (expected, enc) in secrets.iter().zip(&encrypted) {
            let dec = store.decrypt(enc).unwrap();
            assert_eq!(&dec, expected);
        }
    }
}
