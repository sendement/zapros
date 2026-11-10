//! Passwords: in the desktop's secret store (Secret Service: GNOME Keyring,
//! KWallet) when there is one; otherwise in a file of our own, each
//! encrypted with ChaCha20-Poly1305 under a key kept beside it (0600).
//! Calls may wait on the store (an unlock prompt): make them off the UI
//! thread.

use std::collections::BTreeMap;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

const SERVICE: &str = "zapros";

fn entry(id: &str) -> Option<keyring::Entry> {
    // Tests (and a desktop without a keyring) keep to the file.
    if std::env::var_os("ZAPROS_NO_KEYRING").is_some() {
        return None;
    }
    keyring::Entry::new(SERVICE, id).ok()
}

pub fn get(id: &str) -> Option<String> {
    if let Some(e) = entry(id) {
        match e.get_password() {
            Ok(p) => return Some(p),
            Err(keyring::Error::NoEntry) => {}
            Err(_) => {}
        }
    }
    file_get(id)
}

/// Saves it; says where ("keyring" or "file").
pub fn set(id: &str, password: &str) -> Result<&'static str, String> {
    if let Some(e) = entry(id) {
        if e.set_password(password).is_ok() {
            file_remove(id);
            return Ok("keyring");
        }
    }
    file_set(id, password).map(|_| "file")
}

pub fn remove(id: &str) {
    if let Some(e) = entry(id) {
        e.delete_credential().ok();
    }
    file_remove(id);
}

fn dir() -> PathBuf {
    crate::config::data_dir()
}

fn private_write(path: &PathBuf, data: &[u8]) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(tmp, path)
}

fn cipher() -> Option<ChaCha20Poly1305> {
    let path = dir().join("master.key");
    let key = match std::fs::read(&path) {
        Ok(k) if k.len() == 32 => k,
        _ => {
            let mut k = vec![0u8; 32];
            getrandom::fill(&mut k).ok()?;
            private_write(&path, &k).ok()?;
            k
        }
    };
    Some(ChaCha20Poly1305::new(Key::from_slice(&key)))
}

fn load() -> BTreeMap<String, String> {
    std::fs::read_to_string(dir().join("secrets.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

fn store(m: &BTreeMap<String, String>) -> Result<(), String> {
    private_write(&dir().join("secrets.json"), serde_json::to_string_pretty(m).unwrap().as_bytes()).map_err(|e| e.to_string())
}

fn file_get(id: &str) -> Option<String> {
    let b64 = load().get(id)?.clone();
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    if raw.len() < 12 {
        return None;
    }
    let (nonce, data) = raw.split_at(12);
    let plain = cipher()?.decrypt(Nonce::from_slice(nonce), data).ok()?;
    String::from_utf8(plain).ok()
}

fn file_set(id: &str, password: &str) -> Result<(), String> {
    let c = cipher().ok_or("no key")?;
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
    let mut raw = nonce.to_vec();
    raw.extend(c.encrypt(Nonce::from_slice(&nonce), password.as_bytes()).map_err(|e| e.to_string())?);
    let mut m = load();
    m.insert(id.to_string(), base64::engine::general_purpose::STANDARD.encode(raw));
    store(&m)
}

fn file_remove(id: &str) {
    let mut m = load();
    if m.remove(id).is_some() {
        store(&m).ok();
    }
}

#[cfg(test)]
mod tests {
    /// Stores passwords given as ZAPROS_SEED_SECRETS="id=password,…" (for
    /// setting up local test databases): cargo test seed_secrets -- --ignored
    #[test]
    #[ignore]
    fn seed_secrets() {
        let v = std::env::var("ZAPROS_SEED_SECRETS").unwrap_or_default();
        for pair in v.split(',').filter(|p| !p.is_empty()) {
            let (id, pw) = pair.split_once('=').expect("id=password");
            let at = super::set(id, pw).unwrap();
            assert_eq!(super::get(id).as_deref(), Some(pw));
            eprintln!("{id}: {at}");
        }
    }
}
