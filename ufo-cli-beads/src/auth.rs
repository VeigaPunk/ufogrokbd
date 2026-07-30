//! Auth connection cloned from OpenCode pattern.
//! Reads ~/.local/share/opencode/auth.json (or ~/.ufo/auth.json) so pilots
//! can reuse the same credentials the user already has in OpenCode.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthStore {
    /// provider_id -> credential entry (mirrors OpenCode auth.json shape)
    #[serde(flatten)]
    pub providers: HashMap<String, ProviderAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAuth {
    pub type_: Option<String>,
    #[serde(rename = "type", default)]
    pub type_field: Option<String>,
    pub key: Option<String>,
    pub token: Option<String>,
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ProviderAuth {
    #[allow(dead_code)]
    pub fn bearer(&self) -> Option<&str> {
        self.access_token
            .as_deref()
            .or(self.token.as_deref())
            .or(self.key.as_deref())
    }
}

fn opencode_auth_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".local/share/opencode/auth.json"))
}

fn ufo_auth_path() -> Result<PathBuf> {
    let p = dirs::home_dir()
        .context("no home")?
        .join(".ufo")
        .join("auth.json");
    Ok(p)
}

#[allow(dead_code)]
fn set_private_mode(path: &Path, dir: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = if dir { 0o700 } else { 0o600 };
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(mode);
        fs::set_permissions(path, perms)?;
    }
    let _ = (path, dir);
    Ok(())
}

#[allow(dead_code)]
fn atomic_write_string(path: &Path, contents: &str) -> Result<()> {
    let parent = path.parent().context("missing parent directory")?;
    fs::create_dir_all(parent)?;
    set_private_mode(parent, true)?;

    let tmp_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name().and_then(OsStr::to_str).unwrap_or("auth"),
        Uuid::new_v4()
    ));
    {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)?;
        use std::io::Write;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        file.sync_all()?;
    }
    set_private_mode(&tmp_path, false)?;
    fs::rename(&tmp_path, path)?;
    let dir = OpenOptions::new().read(true).open(parent)?;
    dir.sync_all()?;
    set_private_mode(path, false)?;
    Ok(())
}

/// Load auth, preferring OpenCode store then falling back to ~/.ufo/auth.json
pub fn load_auth() -> Result<AuthStore> {
    if let Some(p) = opencode_auth_path() {
        if p.exists() {
            let s = fs::read_to_string(&p).context("read opencode auth")?;
            let store: AuthStore = serde_json::from_str(&s).unwrap_or_default();
            if !store.providers.is_empty() {
                return Ok(store);
            }
        }
    }
    let p = ufo_auth_path()?;
    if p.exists() {
        let s = fs::read_to_string(&p)?;
        return Ok(serde_json::from_str(&s).unwrap_or_default());
    }
    Ok(AuthStore::default())
}

#[allow(dead_code)]
pub fn save_ufo_auth(store: &AuthStore) -> Result<()> {
    let p = ufo_auth_path()?;
    atomic_write_string(&p, &serde_json::to_string_pretty(store)?)
}

pub fn list_providers(store: &AuthStore) -> Vec<(String, String)> {
    store
        .providers
        .iter()
        .map(|(id, a)| {
            let kind = a
                .type_field
                .as_deref()
                .or(a.type_.as_deref())
                .unwrap_or("unknown")
                .to_string();
            (id.clone(), kind)
        })
        .collect()
}
