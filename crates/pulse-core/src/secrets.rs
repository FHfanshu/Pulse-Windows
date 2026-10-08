// Upstream Auth/APIKeyStore.swift (`keys.dat`, AES-GCM with a machine key) becomes
// DPAPI-protected JSON on Windows.
//! Pasted keys and tokens. Changed under one lock, read-modify-write, so a
//! background renewal and a Settings save can never drop each other's entry.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

pub trait SecretStore: Send + Sync {
    fn get(&self, id: &str) -> Option<String>;
    fn set(&self, id: &str, value: Option<&str>);
    /// Replace `id` only while it still holds `expected` (a renewal must not
    /// overwrite a key the reader retyped meanwhile).
    fn replace(&self, id: &str, expected: &str, new: &str) -> bool;
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemorySecrets(Mutex<BTreeMap<String, String>>);

impl SecretStore for MemorySecrets {
    fn get(&self, id: &str) -> Option<String> {
        self.0.lock().unwrap().get(id).cloned()
    }
    fn set(&self, id: &str, value: Option<&str>) {
        let mut map = self.0.lock().unwrap();
        match value {
            Some(v) => map.insert(id.to_string(), v.to_string()),
            None => map.remove(id),
        };
    }
    fn replace(&self, id: &str, expected: &str, new: &str) -> bool {
        let mut map = self.0.lock().unwrap();
        if map.get(id).map(String::as_str) == Some(expected) {
            map.insert(id.to_string(), new.to_string());
            true
        } else {
            false
        }
    }
}

/// `keys.dat`: a JSON map encrypted with DPAPI for the current Windows user.
pub struct FileSecrets {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileSecrets {
    pub fn new(path: PathBuf) -> Self {
        Self { path, lock: Mutex::new(()) }
    }

    fn read(&self) -> BTreeMap<String, String> {
        let Ok(bytes) = std::fs::read(&self.path) else { return BTreeMap::new() };
        dpapi::unprotect(&bytes)
            .and_then(|plain| serde_json::from_slice(&plain).ok())
            .unwrap_or_default()
    }

    fn write(&self, map: &BTreeMap<String, String>) {
        let Ok(plain) = serde_json::to_vec(map) else { return };
        if let Some(sealed) = dpapi::protect(&plain) {
            if let Some(dir) = self.path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let tmp = self.path.with_extension("tmp");
            if std::fs::write(&tmp, sealed).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }
}

impl SecretStore for FileSecrets {
    fn get(&self, id: &str) -> Option<String> {
        let _guard = self.lock.lock().unwrap();
        self.read().get(id).cloned()
    }
    fn set(&self, id: &str, value: Option<&str>) {
        let _guard = self.lock.lock().unwrap();
        let mut map = self.read();
        match value {
            Some(v) => map.insert(id.to_string(), v.to_string()),
            None => map.remove(id),
        };
        self.write(&map);
    }
    fn replace(&self, id: &str, expected: &str, new: &str) -> bool {
        let _guard = self.lock.lock().unwrap();
        let mut map = self.read();
        if map.get(id).map(String::as_str) != Some(expected) {
            return false;
        }
        map.insert(id.to_string(), new.to_string());
        self.write(&map);
        true
    }
}

#[cfg(windows)]
pub mod dpapi {
    use windows::Win32::Foundation::{LocalFree, HLOCAL};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 }
    }

    unsafe fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(out.pbData as _));
        v
    }

    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptProtectData(&blob(plain), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
            Some(take(out))
        }
    }

    pub fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
        let mut out = CRYPT_INTEGER_BLOB::default();
        unsafe {
            CryptUnprotectData(&blob(sealed), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out).ok()?;
            Some(take(out))
        }
    }
}

#[cfg(not(windows))]
pub mod dpapi {
    pub fn protect(plain: &[u8]) -> Option<Vec<u8>> {
        Some(plain.to_vec())
    }
    pub fn unprotect(sealed: &[u8]) -> Option<Vec<u8>> {
        Some(sealed.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_round_trips_and_guards_replace() {
        let dir = std::env::temp_dir().join(format!("pulse-secrets-{}", std::process::id()));
        let store = FileSecrets::new(dir.join("keys.dat"));
        store.set("deepSeek", Some("sk-1"));
        assert_eq!(store.get("deepSeek").as_deref(), Some("sk-1"));
        assert!(!store.replace("deepSeek", "sk-old", "sk-2"));
        assert!(store.replace("deepSeek", "sk-1", "sk-2"));
        assert_eq!(store.get("deepSeek").as_deref(), Some("sk-2"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
