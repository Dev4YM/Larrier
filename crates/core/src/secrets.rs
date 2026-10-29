//! Secret storage backed by the OS keychain (Windows Credential Manager,
//! macOS Keychain, Secret Service), with a local fallback file when no
//! keychain is reachable.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;

const SERVICE: &str = "larrier";

fn fallback_path() -> PathBuf {
    crate::store::data_root().join("secrets.json")
}

fn read_fallback() -> BTreeMap<String, String> {
    std::fs::read_to_string(fallback_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn write_fallback(map: &BTreeMap<String, String>) -> Result<()> {
    let p = fallback_path();
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(p, serde_json::to_string_pretty(map)?)?;
    Ok(())
}

pub fn new_ref(prefix: &str) -> String {
    format!("{prefix}:{}", crate::lsf::new_id())
}

pub fn set(reference: &str, value: &str) -> Result<()> {
    match keyring::Entry::new(SERVICE, reference).and_then(|e| e.set_password(value)) {
        Ok(()) => {
            // Remove a stale fallback copy if any.
            let mut map = read_fallback();
            if map.remove(reference).is_some() {
                let _ = write_fallback(&map);
            }
            Ok(())
        }
        Err(e) => {
            tracing::warn!("keychain unavailable ({e}); storing secret in local file");
            let mut map = read_fallback();
            map.insert(reference.to_string(), value.to_string());
            write_fallback(&map)
        }
    }
}

pub fn get(reference: &str) -> Option<String> {
    if let Ok(v) = keyring::Entry::new(SERVICE, reference).and_then(|e| e.get_password()) {
        return Some(v);
    }
    read_fallback().get(reference).cloned()
}

pub fn delete(reference: &str) {
    if let Ok(e) = keyring::Entry::new(SERVICE, reference) {
        let _ = e.delete_credential();
    }
    let mut map = read_fallback();
    if map.remove(reference).is_some() {
        let _ = write_fallback(&map);
    }
}

/// Resolve `secret:<ref>` placeholders inside env values.
pub fn resolve(value: &str) -> String {
    match value.strip_prefix("secret:") {
        Some(r) => get(r).unwrap_or_default(),
        None => value.to_string(),
    }
}

/// Mask a secret for display: `sk-…abcd`.
pub fn mask(value: &str) -> String {
    let n = value.chars().count();
    if n <= 8 {
        return "••••".into();
    }
    let head: String = value.chars().take(3).collect();
    let tail: String = value.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}
