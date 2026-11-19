//! Terms-of-service approvals (spec 4-3). One record per terms type with the hash of the
//! text that was approved. Records carry an HMAC keyed by a keychain secret, so editing
//! `terms.json` by hand does not count as approval.

use crate::error::{LoaError, Result};
use crate::paths::{Paths, write_atomic};
use crate::secrets::{KEY_TERMS_MAC, SecretStore, get_or_create_key};
use hmac::{Hmac, Mac};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TermsRecord {
    pub terms_id: String,
    pub name: String,
    pub hash: String,
    pub approved_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    pub mac: String,
}

pub struct TermsStore {
    path: PathBuf,
    key: Zeroizing<String>,
}

pub fn hash_text(text: &str) -> String {
    let normalized: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    hex::encode(Sha256::digest(normalized.as_bytes()))
}

impl TermsStore {
    pub fn open(paths: &Paths, secrets: &dyn SecretStore) -> Result<TermsStore> {
        let key = get_or_create_key(secrets, KEY_TERMS_MAC, 32)?;
        Ok(TermsStore {
            path: paths.terms_file(),
            key,
        })
    }

    pub fn with_key(path: PathBuf, key: &str) -> TermsStore {
        TermsStore {
            path,
            key: Zeroizing::new(key.to_string()),
        }
    }

    fn mac(&self, terms_id: &str, hash: &str, at: &str) -> String {
        let mut m = Hmac::<Sha256>::new_from_slice(self.key.as_bytes()).expect("hmac key");
        m.update(format!("{terms_id}\n{hash}\n{at}").as_bytes());
        hex::encode(m.finalize().into_bytes())
    }

    pub fn records(&self) -> Result<Vec<TermsRecord>> {
        match std::fs::read(&self.path) {
            Ok(b) => serde_json::from_slice(&b)
                .map_err(|e| LoaError::other(format!("terms.json を読めません: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn is_valid(&self, r: &TermsRecord) -> bool {
        let expected = self.mac(&r.terms_id, &r.hash, &r.approved_at);
        constant_eq(expected.as_bytes(), r.mac.as_bytes())
    }

    /// Approved with exactly this text?
    pub fn is_approved(&self, terms_id: &str, hash: &str) -> Result<bool> {
        Ok(self
            .records()?
            .iter()
            .any(|r| r.terms_id == terms_id && r.hash == hash && self.is_valid(r)))
    }

    pub fn record(
        &self,
        terms_id: &str,
        name: &str,
        hash: &str,
        link: Option<String>,
    ) -> Result<TermsRecord> {
        let at = crate::util::now_rfc3339();
        let rec = TermsRecord {
            terms_id: terms_id.into(),
            name: name.into(),
            hash: hash.into(),
            approved_at: at.clone(),
            link,
            mac: self.mac(terms_id, hash, &at),
        };
        let mut all: Vec<TermsRecord> = self
            .records()?
            .into_iter()
            .filter(|r| r.terms_id != terms_id)
            .collect();
        all.push(rec.clone());
        write_atomic(&self.path, &serde_json::to_vec_pretty(&all)?)?;
        Ok(rec)
    }
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tag: &str) -> TermsStore {
        let p = std::env::temp_dir().join(format!("loa-terms-{tag}-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        TermsStore::with_key(p, "k")
    }

    #[test]
    fn approve_then_detect_change() {
        let s = store("a");
        let h1 = hash_text("規約 本文 v1");
        assert!(!s.is_approved("t", &h1).unwrap());
        s.record("t", "規約", &h1, None).unwrap();
        assert!(s.is_approved("t", &h1).unwrap());
        assert!(!s.is_approved("t", &hash_text("規約 本文 v2")).unwrap());
    }

    #[test]
    fn whitespace_does_not_change_hash() {
        assert_eq!(hash_text("a  b\n c"), hash_text("a b c"));
    }

    #[test]
    fn forged_record_is_rejected() {
        let s = store("b");
        let h = hash_text("x");
        let forged = vec![TermsRecord {
            terms_id: "t".into(),
            name: "n".into(),
            hash: h.clone(),
            approved_at: "2026-01-01T00:00:00Z".into(),
            link: None,
            mac: "00".repeat(32),
        }];
        std::fs::write(&s.path, serde_json::to_vec(&forged).unwrap()).unwrap();
        assert!(!s.is_approved("t", &h).unwrap());
    }
}
