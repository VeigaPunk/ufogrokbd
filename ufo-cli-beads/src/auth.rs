//! Auth connection cloned from OpenCode pattern.
//! Reads ~/.local/share/opencode/auth.json (or ~/.ufo/auth.json) so pilots
//! can reuse the same credentials the user already has in OpenCode.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthStore {
    /// provider_id -> credential entry (mirrors OpenCode auth.json shape)
    #[serde(flatten)]
    pub providers: HashMap<String, ProviderAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAuth {
    pub type_: Option<String>, // "api" | "oauth" | "wellknown"
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

pub fn save_ufo_auth(store: &AuthStore) -> Result<()> {
    let p = ufo_auth_path()?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(p, serde_json::to_string_pretty(store)?)?;
    Ok(())
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
