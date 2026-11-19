//! `config.toml` (plain text, no secrets).

use crate::error::{LoaError, Result};
use crate::paths::{Paths, write_atomic};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Config {
    /// Login profile used when `--business-id` is omitted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_business_id: Option<String>,
    /// Explicit path to the agent-browser executable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_browser_path: Option<PathBuf>,
    /// Scenario bundle version pinned with `loa scenarios pin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenarios_pin: Option<String>,
    /// Seconds an approval dialog stays open before it counts as denied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_timeout_secs: Option<u64>,
    /// Seconds loa waits for a human (login, SMS) inside a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_timeout_secs: Option<u64>,
    /// Known login profiles.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub business_ids: Vec<String>,
}

pub const DEFAULT_BUSINESS_ID: &str = "default";

impl Config {
    pub fn load(paths: &Paths) -> Result<Config> {
        match std::fs::read_to_string(paths.config_file()) {
            Ok(s) => toml::from_str(&s)
                .map_err(|e| LoaError::other(format!("config.toml を読めません: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let s = toml::to_string_pretty(self)
            .map_err(|e| LoaError::other(format!("config.toml を書けません: {e}")))?;
        write_atomic(&paths.config_file(), s.as_bytes())
    }

    pub fn business_id(&self, explicit: Option<&str>) -> String {
        explicit
            .map(str::to_string)
            .or_else(|| self.default_business_id.clone())
            .unwrap_or_else(|| DEFAULT_BUSINESS_ID.to_string())
    }

    pub fn approval_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.approval_timeout_secs.unwrap_or(600))
    }

    pub fn human_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.human_timeout_secs.unwrap_or(1800))
    }

    pub fn remember_business(&mut self, id: &str) {
        if !self.business_ids.iter().any(|b| b == id) {
            self.business_ids.push(id.to_string());
        }
        if self.default_business_id.is_none() {
            self.default_business_id = Some(id.to_string());
        }
    }
}

/// Business ids become agent-browser session names: letters, digits, `-` and `_` only.
pub fn validate_business_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(LoaError::usage(format!(
            "business-id '{id}' には英数字・ハイフン・アンダースコアのみ使えます（40文字まで）"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let dir = std::env::temp_dir().join(format!("loa-cfg-{}", std::process::id()));
        let paths = Paths::at(&dir);
        let mut c = Config::default();
        c.remember_business("acme");
        c.save(&paths).unwrap();
        let back = Config::load(&paths).unwrap();
        assert_eq!(back.default_business_id.as_deref(), Some("acme"));
        assert_eq!(back.business_id(None), "acme");
        assert_eq!(back.business_id(Some("x")), "x");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn business_id_validation() {
        assert!(validate_business_id("acme_1-x").is_ok());
        assert!(validate_business_id("a b").is_err());
        assert!(validate_business_id("").is_err());
    }
}
