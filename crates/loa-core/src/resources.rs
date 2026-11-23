//! Local map from names to LINE ids (`state/resources.json`, plain text, no secrets).

use crate::error::{LoaError, Result};
use crate::paths::{Paths, write_atomic};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelType {
    Login,
    Messaging,
    Miniapp,
    #[default]
    Unknown,
}

impl ChannelType {
    pub fn parse(s: &str) -> Option<ChannelType> {
        match s {
            "login" => Some(ChannelType::Login),
            "messaging" => Some(ChannelType::Messaging),
            "miniapp" => Some(ChannelType::Miniapp),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelType::Login => "login",
            ChannelType::Messaging => "messaging",
            ChannelType::Miniapp => "miniapp",
            ChannelType::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ChannelRec {
    pub id: String,
    #[serde(rename = "type", default)]
    pub ty: ChannelType,
    /// LIFF app name (description) → LIFF id
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub liff: BTreeMap<String, String>,
    /// MINI App stage → LIFF id
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub miniapp: BTreeMap<String, String>,
    /// Values last applied by `loa apply` (field → value), used to keep apply idempotent.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub applied: BTreeMap<String, String>,
    /// Name shown in the console, set when several channels share it and the map key is the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ProviderRec {
    pub id: String,
    #[serde(default)]
    pub channels: BTreeMap<String, ChannelRec>,
    /// Channel names used by more than one channel in the console (name → ids). Such channels are
    /// keyed by id in `channels` and can only be addressed by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub ambiguous: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct OaRec {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging_channel: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub applied: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Business {
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderRec>,
    #[serde(default)]
    pub official_accounts: BTreeMap<String, OaRec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ResourceMap {
    #[serde(default)]
    pub businesses: BTreeMap<String, Business>,
}

/// A channel found by name.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelHit {
    pub provider: String,
    pub provider_id: String,
    pub name: String,
    pub rec: ChannelRec,
}

impl ResourceMap {
    pub fn load(paths: &Paths) -> Result<ResourceMap> {
        match std::fs::read(paths.resources_file()) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| LoaError::other(format!("resources.json を読めません: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ResourceMap::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        write_atomic(&paths.resources_file(), &serde_json::to_vec_pretty(self)?)
    }

    pub fn business(&mut self, id: &str) -> &mut Business {
        self.businesses.entry(id.to_string()).or_default()
    }

    pub fn business_ref(&self, id: &str) -> Option<&Business> {
        self.businesses.get(id)
    }
}

impl Business {
    pub fn provider_by_name_or_id(&self, key: &str) -> Option<(String, &ProviderRec)> {
        self.providers
            .iter()
            .find(|(n, p)| n.as_str() == key || p.id == key)
            .map(|(n, p)| (n.clone(), p))
    }

    /// Channel by name or id; `provider` narrows the search. Ambiguous names are an error.
    pub fn find_channel(&self, key: &str, provider: Option<&str>) -> Result<Option<ChannelHit>> {
        let mut hits = Vec::new();
        for (pname, p) in &self.providers {
            if let Some(want) = provider
                && pname != want
                && p.id != want
            {
                continue;
            }
            if let Some(ids) = p.ambiguous.get(key) {
                return Err(LoaError::usage(format!(
                    "プロバイダー {pname} にはチャネル名 {key} が {} 個あります（ID: {}）。ID で指定してください",
                    ids.len(),
                    ids.join(", ")
                )));
            }
            for (cname, c) in &p.channels {
                if cname == key || c.id == key {
                    hits.push(ChannelHit {
                        provider: pname.clone(),
                        provider_id: p.id.clone(),
                        name: cname.clone(),
                        rec: c.clone(),
                    });
                }
            }
        }
        match hits.len() {
            0 => Ok(None),
            1 => Ok(hits.pop()),
            _ => Err(LoaError::usage(format!(
                "チャネル名 {key} が複数のプロバイダーにあります。--provider で指定してください"
            ))),
        }
    }

    pub fn upsert_channel(
        &mut self,
        provider: &str,
        provider_id: &str,
        name: &str,
        rec: ChannelRec,
    ) {
        let p = self.providers.entry(provider.to_string()).or_default();
        if !provider_id.is_empty() {
            p.id = provider_id.to_string();
        }
        let entry = p.channels.entry(name.to_string()).or_default();
        let liff = std::mem::take(&mut entry.liff);
        let miniapp = std::mem::take(&mut entry.miniapp);
        let applied = std::mem::take(&mut entry.applied);
        let old_ty = entry.ty;
        *entry = rec;
        if entry.liff.is_empty() {
            entry.liff = liff;
        }
        if entry.miniapp.is_empty() {
            entry.miniapp = miniapp;
        }
        if entry.applied.is_empty() {
            entry.applied = applied;
        }
        if entry.ty == ChannelType::Unknown {
            entry.ty = old_ty;
        }
    }

    pub fn remove_channel(&mut self, provider: &str, name: &str) {
        if let Some(p) = self.providers.get_mut(provider) {
            p.channels.remove(name);
        }
    }

    pub fn channel_mut(&mut self, provider: &str, name: &str) -> Option<&mut ChannelRec> {
        self.providers.get_mut(provider)?.channels.get_mut(name)
    }
}

/// Channel names may not contain "LINE" (LINE Developers Console rule).
pub fn validate_channel_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(LoaError::usage("チャネル名が空です"));
    }
    if name.to_ascii_uppercase().contains("LINE") {
        return Err(LoaError::usage(
            "チャネル名に「LINE」は含められません（LINE Developersコンソールの制約）",
        ));
    }
    if name.chars().count() > 20 {
        return Err(LoaError::usage("チャネル名は20文字以内にしてください"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn biz() -> Business {
        let mut b = Business::default();
        b.upsert_channel(
            "acme",
            "1",
            "app",
            ChannelRec {
                id: "10".into(),
                ty: ChannelType::Login,
                ..Default::default()
            },
        );
        b.upsert_channel(
            "beta",
            "2",
            "app",
            ChannelRec {
                id: "20".into(),
                ty: ChannelType::Login,
                ..Default::default()
            },
        );
        b.upsert_channel(
            "acme",
            "1",
            "bot",
            ChannelRec {
                id: "11".into(),
                ty: ChannelType::Messaging,
                ..Default::default()
            },
        );
        b
    }

    #[test]
    fn find_by_name_id_and_provider() {
        let b = biz();
        assert_eq!(b.find_channel("bot", None).unwrap().unwrap().rec.id, "11");
        assert_eq!(
            b.find_channel("20", None).unwrap().unwrap().provider,
            "beta"
        );
        assert!(b.find_channel("app", None).is_err());
        assert_eq!(
            b.find_channel("app", Some("beta")).unwrap().unwrap().rec.id,
            "20"
        );
        assert!(b.find_channel("nope", None).unwrap().is_none());
    }

    #[test]
    fn shared_channel_name_needs_an_id() {
        let mut b = biz();
        let p = b.providers.get_mut("acme").unwrap();
        for id in ["30", "31"] {
            p.channels.insert(
                id.into(),
                ChannelRec {
                    id: id.into(),
                    display_name: Some("dup".into()),
                    ..Default::default()
                },
            );
        }
        p.ambiguous
            .insert("dup".into(), vec!["30".into(), "31".into()]);
        let e = b.find_channel("dup", Some("acme")).unwrap_err();
        assert!(e.message.contains("30, 31"), "{}", e.message);
        assert_eq!(
            b.find_channel("31", Some("acme")).unwrap().unwrap().rec.id,
            "31"
        );
    }

    #[test]
    fn upsert_keeps_liff_map() {
        let mut b = biz();
        b.channel_mut("acme", "app")
            .unwrap()
            .liff
            .insert("main".into(), "L1".into());
        b.upsert_channel(
            "acme",
            "1",
            "app",
            ChannelRec {
                id: "10".into(),
                ty: ChannelType::Login,
                ..Default::default()
            },
        );
        assert_eq!(
            b.find_channel("app", Some("acme"))
                .unwrap()
                .unwrap()
                .rec
                .liff["main"],
            "L1"
        );
    }

    #[test]
    fn channel_name_rules() {
        assert!(validate_channel_name("acme-liff-stg").is_ok());
        assert!(validate_channel_name("my LINE app").is_err());
        assert!(validate_channel_name("").is_err());
    }
}
