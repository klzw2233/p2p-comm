use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use argon2::Argon2;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use crate::inbox::ChatMessage;
#[cfg(test)]
use crate::inbox::Direction;
use crate::Error;

const SALT_FILE: &str = "chat.salt";
const SALT_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const ARGON2_M_KIB: u32 = 19 * 1024;
const ARGON2_T: u32 = 2;
const ARGON2_P: u32 = 1;

/// Per-device chat master key. Derived once from the identity password.
pub struct ChatKeys {
    master: [u8; 32],
    dir: PathBuf,
}

impl ChatKeys {
    /// Load or create `{dir}/chat.salt`, then Argon2id(password, salt) → master.
    ///
    /// # Errors
    ///
    /// * [`Error::EmptyPassword`]
    /// * [`Error::Io`] if the salt file cannot be read or written
    /// * [`Error::CorruptStore`] if the salt is the wrong length
    pub fn unlock(dir: &Path, password: &str) -> Result<Self, Error> {
        if password.is_empty() {
            return Err(Error::EmptyPassword);
        }
        fs::create_dir_all(dir).map_err(|_| Error::Io)?;
        let salt_path = dir.join(SALT_FILE);
        let salt = match fs::read(&salt_path) {
            Ok(bytes) if bytes.len() == SALT_LEN => {
                let mut salt = [0u8; SALT_LEN];
                salt.copy_from_slice(&bytes);
                salt
            }
            Ok(_) => return Err(Error::CorruptStore),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let mut salt = [0u8; SALT_LEN];
                rand::thread_rng().fill_bytes(&mut salt);
                fs::write(&salt_path, salt).map_err(|_| Error::Io)?;
                salt
            }
            Err(_) => return Err(Error::Io),
        };
        Ok(Self {
            master: derive_master(password.as_bytes(), &salt)?,
            dir: dir.to_path_buf(),
        })
    }

    /// Load every stored message for `peer_id_hex`. Missing file → empty.
    ///
    /// # Errors
    ///
    /// * [`Error::DecryptFailed`] if a line cannot be decrypted with this password
    /// * [`Error::Io`] / [`Error::CorruptStore`]
    pub fn load(&self, peer_id_hex: &str) -> Result<Vec<ChatMessage>, Error> {
        let path = self.path_for(peer_id_hex);
        let file = match fs::File::open(&path) {
            Ok(f) => f,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(Error::Io),
        };
        let key = self.peer_key(peer_id_hex)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let mut out = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line.map_err(|_| Error::Io)?;
            if line.is_empty() {
                continue;
            }
            out.push(decrypt_line(&cipher, &line)?);
        }
        Ok(out)
    }

    /// Append one message. Creates the per-peer JSONL file if needed.
    ///
    /// # Errors
    ///
    /// * [`Error::Io`]
    pub fn append(&self, peer_id_hex: &str, msg: &ChatMessage) -> Result<(), Error> {
        let path = self.path_for(peer_id_hex);
        let key = self.peer_key(peer_id_hex)?;
        let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
        let line = encrypt_line(&cipher, msg)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|_| Error::Io)?;
        writeln!(file, "{line}").map_err(|_| Error::Io)?;
        file.sync_all().map_err(|_| Error::Io)
    }

    fn path_for(&self, peer_id_hex: &str) -> PathBuf {
        self.dir.join(format!("{peer_id_hex}.jsonl"))
    }

    fn peer_key(&self, peer_id_hex: &str) -> Result<[u8; 32], Error> {
        let hk = Hkdf::<Sha256>::from_prk(&self.master).map_err(|_| Error::CorruptStore)?;
        let mut out = [0u8; 32];
        hk.expand(peer_id_hex.as_bytes(), &mut out)
            .map_err(|_| Error::CorruptStore)?;
        Ok(out)
    }
}

fn derive_master(password: &[u8], salt: &[u8; SALT_LEN]) -> Result<[u8; 32], Error> {
    let params = argon2::Params::new(ARGON2_M_KIB, ARGON2_T, ARGON2_P, Some(32))
        .map_err(|_| Error::CorruptStore)?;
    let argon = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password, salt, &mut out)
        .map_err(|_| Error::CorruptStore)?;
    Ok(out)
}

fn encrypt_line(cipher: &ChaCha20Poly1305, msg: &ChatMessage) -> Result<String, Error> {
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let plain = serde_json::to_vec(msg).map_err(|_| Error::Io)?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plain.as_ref())
        .map_err(|_| Error::Io)?;
    let mut packed = Vec::with_capacity(NONCE_LEN + ct.len());
    packed.extend_from_slice(&nonce_bytes);
    packed.extend_from_slice(&ct);
    Ok(to_hex(&packed))
}

fn decrypt_line(cipher: &ChaCha20Poly1305, line: &str) -> Result<ChatMessage, Error> {
    let packed = from_hex(line.trim()).ok_or(Error::CorruptStore)?;
    if packed.len() < NONCE_LEN + 16 {
        return Err(Error::CorruptStore);
    }
    let nonce = Nonce::from_slice(&packed[..NONCE_LEN]);
    let plain = cipher
        .decrypt(nonce, packed[NONCE_LEN..].as_ref())
        .map_err(|_| Error::DecryptFailed)?;
    serde_json::from_slice(&plain).map_err(|_| Error::CorruptStore)
}

fn to_hex(bytes: &[u8]) -> String {
    crate::to_hex(bytes)
}

fn from_hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = Vec::with_capacity(s.len() / 2);
    for chunk in s.as_bytes().chunks_exact(2) {
        let hex = std::str::from_utf8(chunk).ok()?;
        out.push(u8::from_str_radix(hex, 16).ok()?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::{temp_path, valid_peer_hex};

    fn msg(content: &str, ts: u64, outgoing: bool) -> ChatMessage {
        ChatMessage {
            content: content.to_owned(),
            timestamp: ts,
            direction: if outgoing {
                Direction::Outgoing
            } else {
                Direction::Incoming
            },
            failed: false,
        }
    }

    #[test]
    fn roundtrip_and_restart() {
        let dir = temp_path();
        let peer = valid_peer_hex();
        let keys = ChatKeys::unlock(&dir, "correct-horse").expect("unlock");
        keys.append(peer.as_str(), &msg("hi", 1, true)).expect("append");
        keys.append(peer.as_str(), &msg("yo", 2, false)).expect("append");
        let loaded = keys.load(peer.as_str()).expect("load");
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].content, "hi");
        assert_eq!(loaded[1].content, "yo");

        let again = ChatKeys::unlock(&dir, "correct-horse").expect("reunlock");
        let loaded = again.load(peer.as_str()).expect("reload");
        assert_eq!(loaded[0].content, "hi");
        assert_eq!(loaded[1].direction, Direction::Incoming);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_is_not_plaintext() {
        let dir = temp_path();
        let peer = valid_peer_hex();
        let keys = ChatKeys::unlock(&dir, "correct-horse").expect("unlock");
        keys.append(peer.as_str(), &msg("secret-payload", 1, true))
            .expect("append");
        let raw = fs::read_to_string(dir.join(format!("{peer}.jsonl"))).expect("read");
        assert!(!raw.contains("secret-payload"));
        assert!(!raw.contains("Outgoing"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn peers_are_isolated() {
        let dir = temp_path();
        let a = valid_peer_hex();
        let b = valid_peer_hex();
        let keys = ChatKeys::unlock(&dir, "correct-horse").expect("unlock");
        keys.append(a.as_str(), &msg("for-a", 1, true)).expect("a");
        keys.append(b.as_str(), &msg("for-b", 1, true)).expect("b");
        assert_eq!(keys.load(a.as_str()).expect("a")[0].content, "for-a");
        assert_eq!(keys.load(b.as_str()).expect("b")[0].content, "for-b");
        assert_eq!(keys.load(a.as_str()).expect("a").len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wrong_password_cannot_decrypt() {
        let dir = temp_path();
        let peer = valid_peer_hex();
        let keys = ChatKeys::unlock(&dir, "correct-horse").expect("unlock");
        keys.append(peer.as_str(), &msg("hi", 1, true)).expect("append");
        let wrong = ChatKeys::unlock(&dir, "wrong-battery").expect("wrong still opens salt");
        assert_eq!(wrong.load(peer.as_str()).unwrap_err(), Error::DecryptFailed);
        let _ = fs::remove_dir_all(&dir);
    }
}
