use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use p2p_trust::PeerId;

use crate::{parse_peer_id_hex, PeerIdHex, Error};

const FILE_NAME: &str = "nicknames.json";

/// Local-only nickname table. Never sent on the wire.
pub struct NicknameStore {
    path: PathBuf,
    by_peer: BTreeMap<PeerIdHex, String>,
}

impl NicknameStore {
    /// Load `{dir}/nicknames.json`, or start empty if the file is missing.
    ///
    /// # Errors
    ///
    /// * [`Error::Io`] if the file cannot be read
    /// * [`Error::CorruptStore`] if the JSON is not `{peer_id: nickname}`
    pub fn load(dir: &Path) -> Result<Self, Error> {
        let path = dir.join(FILE_NAME);
        let by_peer = match fs::read(&path) {
            Ok(bytes) => parse_file(&bytes)?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(_) => return Err(Error::Io),
        };
        Ok(Self { path, by_peer })
    }

    /// Nickname for `peer_id_hex`, if any.
    #[must_use]
    pub fn get(&self, peer_id_hex: &PeerIdHex) -> Option<&str> {
        self.by_peer.get(peer_id_hex).map(String::as_str)
    }

    /// Nickname if set, otherwise the first 8 hex characters of the Peer ID.
    #[must_use]
    pub fn display_name(&self, peer_id_hex: &PeerIdHex) -> String {
        self.get(peer_id_hex)
            .map_or_else(|| short_id(peer_id_hex.as_str()), ToOwned::to_owned)
    }

    /// Reverse lookup: nickname → 64-char hex Peer ID.
    #[must_use]
    pub fn peer_id_hex_for_nickname(&self, nickname: &str) -> Option<&PeerIdHex> {
        self.by_peer
            .iter()
            .find_map(|(peer, name)| (name == nickname).then_some(peer))
    }

    /// Iterate `(peer_id_hex, nickname)` in Peer ID order.
    pub fn iter(&self) -> impl Iterator<Item = (&PeerIdHex, &str)> {
        self.by_peer
            .iter()
            .map(|(peer, name)| (peer, name.as_str()))
    }

    /// Set or replace the nickname for `peer_id_hex`. Writes immediately.
    ///
    /// # Errors
    ///
    /// * [`Error::InvalidPeerId`] if `peer_id_hex` is not a 64-char hex Peer ID
    /// * [`Error::EmptyNickname`] if `nickname` is empty
    /// * [`Error::DuplicateNickname`] if another Peer already has this nickname
    /// * [`Error::Io`] if the file cannot be written
    pub fn set(&mut self, peer_id_hex: &PeerIdHex, nickname: &str) -> Result<(), Error> {
        let nickname = nickname.trim();
        if nickname.is_empty() {
            return Err(Error::EmptyNickname);
        }
        if self
            .by_peer
            .iter()
            .any(|(peer, name)| peer != peer_id_hex && name == nickname)
        {
            return Err(Error::DuplicateNickname);
        }
        self.by_peer.insert(peer_id_hex.clone(), nickname.to_owned());
        self.persist()
    }

    /// Remove the nickname for `peer_id_hex`. Writes immediately. Missing is ok.
    ///
    /// # Errors
    ///
    /// * [`Error::Io`] if the file cannot be written
    pub fn remove(&mut self, peer_id_hex: &PeerIdHex) -> Result<(), Error> {
        self.by_peer.remove(peer_id_hex);
        self.persist()
    }
}

/// Resolve a dial box value: 64-char hex Peer ID, or an existing local nickname.
///
/// # Errors
///
/// * [`Error::InvalidPeerId`] if the input looks like hex but is not a valid Peer ID
/// * [`Error::UnknownNickname`] if it is not hex and not a stored nickname
pub fn resolve_dial(store: &NicknameStore, input: &str) -> Result<(PeerId, PeerIdHex), Error> {
    let input = input.trim();
    if input.is_empty() {
        return Err(Error::InvalidPeerId);
    }
    if input.len() == 64 {
        return parse_peer_id_hex(input);
    }
    store
        .peer_id_hex_for_nickname(input)
        .ok_or(Error::UnknownNickname)
        .and_then(|hex| parse_peer_id_hex(hex.as_str()))
}

#[must_use]
pub fn short_id(peer_id_hex: &str) -> String {
    peer_id_hex.chars().take(8).collect()
}

fn parse_file(bytes: &[u8]) -> Result<BTreeMap<PeerIdHex, String>, Error> {
    let raw: BTreeMap<String, String> =
        serde_json::from_slice(bytes).map_err(|_| Error::CorruptStore)?;
    let mut by_peer = BTreeMap::new();
    for (peer, name) in raw {
        let (_, hex) = parse_peer_id_hex(&peer).map_err(|_| Error::CorruptStore)?;
        if name.trim().is_empty() {
            return Err(Error::CorruptStore);
        }
        by_peer.insert(hex, name);
    }
    Ok(by_peer)
}

fn persist_map(path: &Path, by_peer: &BTreeMap<PeerIdHex, String>) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| Error::Io)?;
    }
    // Serialize PeerIdHex keys as strings
    let string_map: BTreeMap<String, String> = by_peer
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect();
    let json = serde_json::to_vec_pretty(&string_map).map_err(|_| Error::Io)?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp).map_err(|_| Error::Io)?;
        file.write_all(&json).map_err(|_| Error::Io)?;
        file.sync_all().map_err(|_| Error::Io)?;
    }
    // Windows cannot rename over an existing file.
    if path.exists() {
        fs::remove_file(path).map_err(|_| Error::Io)?;
    }
    fs::rename(&tmp, path).map_err(|_| Error::Io)
}

impl NicknameStore {
    fn persist(&self) -> Result<(), Error> {
        persist_map(&self.path, &self.by_peer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests_support::{temp_path, valid_peer_hex};
    use p2p_trust::IdentityKey;

    fn two_peers() -> (PeerIdHex, PeerIdHex) {
        let a = valid_peer_hex();
        let b = loop {
            let hex = valid_peer_hex();
            if hex != a {
                break hex;
            }
        };
        (a, b)
    }

    #[test]
    fn missing_file_is_empty() {
        let dir = temp_path();
        let store = NicknameStore::load(&dir).expect("load");
        let fake_id = valid_peer_hex();
        assert!(store.get(&fake_id).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_get_remove_and_restart() {
        let dir = temp_path();
        let peer = IdentityKey::generate().peer_id();
        let hex = crate::to_hex(peer.as_bytes()).parse::<PeerIdHex>().expect("parse");
        let mut store = NicknameStore::load(&dir).expect("load");
        store.set(&hex, "Alice").expect("set");
        assert_eq!(store.get(&hex), Some("Alice"));
        assert_eq!(store.display_name(&hex), "Alice");

        let reloaded = NicknameStore::load(&dir).expect("reload");
        assert_eq!(reloaded.get(&hex), Some("Alice"));

        store.set(&hex, "Alicia").expect("rename");
        assert_eq!(store.get(&hex), Some("Alicia"));
        let reloaded = NicknameStore::load(&dir).expect("reload rename");
        assert_eq!(reloaded.get(&hex), Some("Alicia"));

        store.remove(&hex).expect("remove");
        assert!(store.get(&hex).is_none());
        assert_eq!(store.display_name(&hex), short_id(hex.as_str()));
        let reloaded = NicknameStore::load(&dir).expect("reload after delete");
        assert!(reloaded.get(&hex).is_none());
        assert_eq!(reloaded.display_name(&hex), hex.as_str()[..8].to_owned());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_peer_displays_short_id() {
        let dir = temp_path();
        let store = NicknameStore::load(&dir).expect("load");
        let hex = valid_peer_hex();
        assert_eq!(store.display_name(&hex), &hex.as_str()[..8]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_or_duplicate_nickname_is_rejected() {
        let dir = temp_path();
        let (a, b) = two_peers();
        let a_id = a;
        let b_id = b;
        let mut store = NicknameStore::load(&dir).expect("load");
        assert_eq!(store.set(&a_id, "  ").unwrap_err(), Error::EmptyNickname);
        store.set(&a_id, "Alice").expect("set");
        assert_eq!(
            store.set(&b_id, "Alice").unwrap_err(),
            Error::DuplicateNickname
        );
        store.set(&a_id, "Alice").expect("same peer rename to self ok");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dial_hex_or_nickname() {
        let dir = temp_path();
        let a = valid_peer_hex();
        let mut store = NicknameStore::load(&dir).expect("load");
        store.set(&a, "Alice").expect("set");

        let (_, hex) = resolve_dial(&store, a.as_str()).expect("hex");
        assert_eq!(hex, a);
        let (_, hex) = resolve_dial(&store, "Alice").expect("nick");
        assert_eq!(hex, a);
        assert_eq!(
            resolve_dial(&store, "Bob").unwrap_err(),
            Error::UnknownNickname
        );
        assert_eq!(
            resolve_dial(&store, "zzzz").unwrap_err(),
            Error::UnknownNickname
        );
        assert_eq!(
            resolve_dial(&store, &"g".repeat(64)).unwrap_err(),
            Error::InvalidPeerId
        );
        assert_eq!(resolve_dial(&store, "").unwrap_err(), Error::InvalidPeerId);
        assert_eq!(
            resolve_dial(&store, &"ab".repeat(31)).unwrap_err(),
            Error::UnknownNickname
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hex_is_case_insensitive() {
        let dir = temp_path();
        let store = NicknameStore::load(&dir).expect("load");
        let hex = valid_peer_hex();
        let upper = hex.as_str().to_ascii_uppercase();
        let (_, got) = resolve_dial(&store, &upper).expect("upper");
        assert_eq!(got, hex);
        let _ = fs::remove_dir_all(&dir);
    }
}
