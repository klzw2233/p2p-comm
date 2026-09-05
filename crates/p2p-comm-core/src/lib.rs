use std::path::{Path, PathBuf};

use p2p_trust::{FileKeyStore, IdentityKey, KeyStore, TrustError};

/// Unlocked local identity. The secret seed is not retained.
#[derive(Debug)]
pub struct Identity {
    peer_id_hex: String,
}

impl Identity {
    /// 64-character lowercase hex encoding of this device's Peer ID.
    #[must_use]
    pub fn peer_id_hex(&self) -> &str {
        &self.peer_id_hex
    }
}

/// Failures when creating or unlocking a local identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    EmptyPassword,
    WrongPassword,
    DataDirUnavailable,
    Io,
    CorruptStore,
}

/// Platform data directory (`~/.local/share/p2p-comm` on Linux, `%APPDATA%\p2p-comm` on Windows).
///
/// # Errors
///
/// Returns [`Error::DataDirUnavailable`] if the platform data directory cannot be resolved.
pub fn default_data_dir() -> Result<PathBuf, Error> {
    dirs::data_dir()
        .map(|dir| dir.join("p2p-comm"))
        .ok_or(Error::DataDirUnavailable)
}

/// Whether an encrypted identity already exists in the platform data directory.
#[must_use]
pub fn has_stored_identity() -> bool {
    default_data_dir().is_ok_and(|dir| has_stored_identity_in(&dir))
}

/// Whether an encrypted identity already exists under `dir`.
#[must_use]
pub fn has_stored_identity_in(dir: &Path) -> bool {
    dir.join("identity.key").is_file()
}

/// Unlock or create the identity in the platform data directory.
///
/// # Errors
///
/// See [`unlock_in`].
pub fn unlock(password: &str) -> Result<Identity, Error> {
    unlock_in(&default_data_dir()?, password)
}

/// Unlock an existing identity under `dir`, or create one if none is stored.
///
/// Only generates a new Identity Key when the store is empty. A wrong password
/// or corrupt file does not overwrite the stored key.
///
/// # Errors
///
/// * [`Error::EmptyPassword`] if `password` is empty
/// * [`Error::WrongPassword`] if a stored identity exists but cannot be decrypted
/// * [`Error::Io`] if the directory or key file cannot be written
/// * [`Error::CorruptStore`] if the key file is malformed
pub fn unlock_in(dir: &Path, password: &str) -> Result<Identity, Error> {
    if password.is_empty() {
        return Err(Error::EmptyPassword);
    }
    std::fs::create_dir_all(dir).map_err(|_| Error::Io)?;
    let mut store = FileKeyStore::new(dir, password.as_bytes());
    let key = if let Some(key) = store.load().map_err(map_trust)? {
        key
    } else {
        let key = IdentityKey::generate();
        store.save(&key).map_err(map_trust)?;
        key
    };
    Ok(Identity {
        peer_id_hex: to_hex(key.peer_id().as_bytes()),
    })
}

fn map_trust(err: TrustError) -> Error {
    match err {
        TrustError::WrongPassword => Error::WrongPassword,
        TrustError::Io => Error::Io,
        TrustError::CorruptStore | TrustError::InvalidPublicKey | TrustError::UnknownPeer => {
            Error::CorruptStore
        }
    }
}

fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static SEQ: AtomicU64 = AtomicU64::new(0);

    fn temp_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "p2p-comm-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn empty_password_is_rejected() {
        let dir = temp_path();
        let err = unlock_in(&dir, "").expect_err("empty password");
        assert_eq!(err, Error::EmptyPassword);
        assert!(!dir.join("identity.key").exists());
    }

    #[test]
    fn new_user_creates_encrypted_identity() {
        let dir = temp_path();
        let identity = unlock_in(&dir, "correct-horse").expect("create");
        assert_eq!(identity.peer_id_hex().len(), 64);
        assert!(identity
            .peer_id_hex()
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        assert!(dir.is_dir());
        let key_path = dir.join("identity.key");
        let bytes = std::fs::read(&key_path).expect("key file");
        assert!(bytes.starts_with(b"P2PKEY01"));
        assert!(bytes.len() > 8 + 16 + 12 + 16);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_password_unlocks_same_identity() {
        let dir = temp_path();
        let first = unlock_in(&dir, "correct-horse").expect("create");
        let second = unlock_in(&dir, "correct-horse").expect("unlock");
        assert_eq!(first.peer_id_hex(), second.peer_id_hex());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_password_does_not_replace_identity() {
        let dir = temp_path();
        let first = unlock_in(&dir, "correct-horse").expect("create");
        let err = unlock_in(&dir, "wrong-battery").expect_err("wrong password");
        assert_eq!(err, Error::WrongPassword);
        let again = unlock_in(&dir, "correct-horse").expect("still unlocks");
        assert_eq!(first.peer_id_hex(), again.peer_id_hex());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_data_dir_is_platform_p2p_comm() {
        let dir = default_data_dir().expect("data dir");
        let expected = dirs::data_dir()
            .expect("platform data dir")
            .join("p2p-comm");
        assert_eq!(dir, expected);
    }

    #[test]
    fn has_stored_identity_tracks_key_file() {
        let dir = temp_path();
        assert!(!has_stored_identity_in(&dir));
        unlock_in(&dir, "correct-horse").expect("create");
        assert!(has_stored_identity_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
