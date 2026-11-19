//! Secret storage. Production uses the OS keychain (macOS Keychain / Windows Credential Manager).
//! Values never go to stdout; callers get a `secret_ref` instead (spec 3-4).

use crate::error::{Code, LoaError, Result};
use std::sync::mpsc;
use std::time::Duration;
use zeroize::Zeroizing;

pub const SERVICE: &str = "loa";
pub const KEY_BROWSER_ENCRYPTION: &str = "_loa/browser/encryption_key";
pub const KEY_TERMS_MAC: &str = "_loa/terms/mac_key";

/// Keychain calls can block on an OS permission prompt (unsigned binaries after an update).
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(90);

pub trait SecretStore: Send + Sync {
    fn set(&self, key: &str, value: &str) -> Result<()>;
    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>>;
    fn delete(&self, key: &str) -> Result<()>;
}

/// `loa://<provider>/<channel>/<key>`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    pub provider: String,
    pub channel: String,
    pub key: String,
}

impl SecretRef {
    pub fn new(provider: &str, channel: &str, key: &str) -> Self {
        SecretRef {
            provider: provider.into(),
            channel: channel.into(),
            key: key.into(),
        }
    }
    pub fn uri(&self) -> String {
        format!("loa://{}/{}/{}", self.provider, self.channel, self.key)
    }
    pub fn store_key(&self) -> String {
        format!("{}/{}/{}", self.provider, self.channel, self.key)
    }
    pub fn parse(uri: &str) -> Option<SecretRef> {
        let rest = uri.strip_prefix("loa://")?;
        let mut it = rest.splitn(3, '/');
        let (p, c, k) = (it.next()?, it.next()?, it.next()?);
        if p.is_empty() || c.is_empty() || k.is_empty() {
            return None;
        }
        Some(SecretRef::new(p, c, k))
    }
}

pub struct KeychainStore;

impl KeychainStore {
    pub fn open() -> Result<KeychainStore> {
        init_default_store()?;
        Ok(KeychainStore)
    }

    fn run<T: Send + 'static>(
        &self,
        key: &str,
        f: impl FnOnce(keyring_core::Entry) -> keyring_core::Result<T> + Send + 'static,
    ) -> Result<keyring_core::Result<T>> {
        let entry = keyring_core::Entry::new(SERVICE, key)
            .map_err(|e| LoaError::other(format!("キーチェーンの項目を準備できません: {e}")))?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f(entry));
        });
        rx.recv_timeout(KEYCHAIN_TIMEOUT).map_err(|_| {
            LoaError::new(Code::HumanActionRequired, "キーチェーンの応答がありません")
                .with_hint("OSのキーチェーン許可ダイアログが出ていれば「常に許可」を選んでください")
        })
    }
}

#[cfg(target_os = "macos")]
fn init_default_store() -> Result<()> {
    use std::sync::Once;
    static INIT: Once = Once::new();
    let mut err = None;
    INIT.call_once(
        || match apple_native_keyring_store::keychain::Store::new() {
            Ok(s) => keyring_core::set_default_store(s),
            Err(e) => err = Some(e.to_string()),
        },
    );
    match err {
        Some(e) => Err(LoaError::setup(format!("キーチェーンを開けません: {e}"))),
        None => Ok(()),
    }
}

#[cfg(windows)]
fn init_default_store() -> Result<()> {
    use std::sync::Once;
    static INIT: Once = Once::new();
    let mut err = None;
    INIT.call_once(|| match windows_native_keyring_store::Store::new() {
        Ok(s) => keyring_core::set_default_store(s),
        Err(e) => err = Some(e.to_string()),
    });
    match err {
        Some(e) => Err(LoaError::setup(format!(
            "資格情報マネージャーを開けません: {e}"
        ))),
        None => Ok(()),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn init_default_store() -> Result<()> {
    Err(LoaError::setup(
        "このOSのキーチェーンには対応していません（macOS / Windows のみ）",
    ))
}

impl SecretStore for KeychainStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let v = Zeroizing::new(value.to_string());
        self.run(key, move |e| e.set_password(&v))?
            .map_err(|e| LoaError::other(format!("キーチェーンへ保存できません: {e}")))
    }

    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
        match self.run(key, |e| e.get_password())? {
            Ok(v) => {
                crate::redact::register_secret(&v);
                Ok(Some(Zeroizing::new(v)))
            }
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(LoaError::other(format!(
                "キーチェーンから読み取れません: {e}"
            ))),
        }
    }

    fn delete(&self, key: &str) -> Result<()> {
        match self.run(key, |e| e.delete_credential())? {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(LoaError::other(format!(
                "キーチェーンから削除できません: {e}"
            ))),
        }
    }
}

/// File backed store for tests that span processes (worker + client). test-hooks only.
#[cfg(feature = "test-hooks")]
pub struct FileStore {
    path: std::path::PathBuf,
}

#[cfg(feature = "test-hooks")]
impl FileStore {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        FileStore { path: path.into() }
    }
    fn load(&self) -> std::collections::BTreeMap<String, String> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self, m: &std::collections::BTreeMap<String, String>) -> Result<()> {
        crate::paths::write_atomic(&self.path, &serde_json::to_vec(m)?)
    }
}

#[cfg(feature = "test-hooks")]
impl SecretStore for FileStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        let mut m = self.load();
        m.insert(key.into(), value.into());
        self.save(&m)
    }
    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
        Ok(self.load().get(key).map(|v| {
            crate::redact::register_secret(v);
            Zeroizing::new(v.clone())
        }))
    }
    fn delete(&self, key: &str) -> Result<()> {
        let mut m = self.load();
        m.remove(key);
        self.save(&m)
    }
}

/// In-memory store for unit tests.
#[derive(Default)]
pub struct MemoryStore {
    inner: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
}

impl SecretStore for MemoryStore {
    fn set(&self, key: &str, value: &str) -> Result<()> {
        self.inner.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Option<Zeroizing<String>>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .get(key)
            .map(|v| Zeroizing::new(v.clone())))
    }
    fn delete(&self, key: &str) -> Result<()> {
        self.inner.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The store used by the CLI. `LOA_TEST_SECRET_FILE` switches to a file store in test builds.
pub fn open_default() -> Result<Box<dyn SecretStore>> {
    #[cfg(feature = "test-hooks")]
    if let Ok(p) = std::env::var("LOA_TEST_SECRET_FILE") {
        return Ok(Box::new(FileStore::new(p)));
    }
    Ok(Box::new(KeychainStore::open()?))
}

/// Read a random hex key, creating it on first use.
pub fn get_or_create_key(
    store: &dyn SecretStore,
    key: &str,
    bytes: usize,
) -> Result<Zeroizing<String>> {
    if let Some(v) = store.get(key)? {
        return Ok(v);
    }
    let mut buf = Zeroizing::new(vec![0u8; bytes]);
    getrandom::fill(&mut buf).map_err(|e| LoaError::other(format!("乱数を生成できません: {e}")))?;
    let hexed = Zeroizing::new(hex::encode(&*buf));
    store.set(key, &hexed)?;
    Ok(hexed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_ref_roundtrip() {
        let r = SecretRef::new("acme", "acme-liff-stg", "channel_secret");
        assert_eq!(r.uri(), "loa://acme/acme-liff-stg/channel_secret");
        assert_eq!(SecretRef::parse(&r.uri()), Some(r));
        assert_eq!(SecretRef::parse("loa://a/b"), None);
    }

    #[test]
    fn key_created_once() {
        let s = MemoryStore::default();
        let a = get_or_create_key(&s, "k", 32).unwrap();
        let b = get_or_create_key(&s, "k", 32).unwrap();
        assert_eq!(*a, *b);
        assert_eq!(a.len(), 64);
    }
}
