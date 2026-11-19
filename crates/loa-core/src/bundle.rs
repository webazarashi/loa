//! Scenario bundles from GitHub Releases (spec 7-3, v1.1 change 7).
//! A bundle is `loa-scenarios-<ver>.tar.gz` with `.sha256` and `.minisig` next to it.
//! Both the checksum and the minisign signature (public key compiled in) must match.

use crate::error::{LoaError, Result};
use crate::paths::{Paths, write_atomic};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// `owner/repo`, set at build time. Unset → `scenarios update` is disabled.
pub const REPO: Option<&str> = option_env!("LOA_SCENARIO_REPO");
/// minisign public key (base64), set at build time.
pub const PUBKEY: Option<&str> = option_env!("LOA_SCENARIO_PUBKEY");

const MAX_BUNDLE_BYTES: u64 = 5 * 1024 * 1024;
/// Limits after decompression (a small signed gzip must not expand without bound).
const MAX_FILE_BYTES: u64 = 512 * 1024;
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
const MAX_FILES: usize = 1000;

fn active_file(paths: &Paths) -> std::path::PathBuf {
    paths.scenarios_dir().join("ACTIVE")
}

/// Installed bundle in use: the pin if installed, otherwise the last updated one.
pub fn active_version(paths: &Paths, pin: Option<&str>) -> Option<String> {
    let installed = |v: &str| paths.scenarios_dir().join(v).join("VERSION").is_file();
    if let Some(p) = pin {
        return installed(p).then(|| p.to_string());
    }
    let v = std::fs::read_to_string(active_file(paths)).ok()?;
    let v = v.trim().to_string();
    installed(&v).then_some(v)
}

pub fn installed_versions(paths: &Paths) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(paths.scenarios_dir())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().join("VERSION").is_file())
                .filter_map(|e| e.file_name().into_string().ok())
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| {
        let pa = semver::Version::parse(a).ok();
        let pb = semver::Version::parse(b).ok();
        pa.cmp(&pb)
    });
    v
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

fn config() -> Result<(&'static str, &'static str)> {
    match (REPO, PUBKEY) {
        (Some(r), Some(k)) if !r.is_empty() && !k.is_empty() => Ok((r, k)),
        _ => Err(LoaError::setup(
            "このビルドにはシナリオ配布元（LOA_SCENARIO_REPO / LOA_SCENARIO_PUBKEY）が設定されていません",
        )
        .with_hint("同梱シナリオはそのまま使えます。配布元を設定したビルドで loa scenarios update を実行してください")),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .user_agent(format!("loa/{}", crate::util::LOA_VERSION))
        .build()
        .into()
}

fn get_bytes(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>> {
    let mut resp = agent
        .get(url)
        .call()
        .map_err(|e| LoaError::other(format!("ダウンロードに失敗しました: {e}")))?;
    let mut buf = Vec::new();
    resp.body_mut()
        .as_reader()
        .take(MAX_BUNDLE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| LoaError::other(format!("ダウンロードに失敗しました: {e}")))?;
    if buf.len() as u64 > MAX_BUNDLE_BYTES {
        return Err(LoaError::other("シナリオの配布物が大きすぎます"));
    }
    Ok(buf)
}

/// Download, verify and install a bundle. `want` = specific version or newest.
pub fn update(paths: &Paths, want: Option<&str>) -> Result<String> {
    let (repo, pubkey) = config()?;
    let agent = agent();
    let api = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    let mut resp = agent
        .get(&api)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| LoaError::other(format!("GitHub Releases を取得できません: {e}")))?;
    let releases: Vec<Release> = resp
        .body_mut()
        .read_json()
        .map_err(|e| LoaError::other(format!("GitHub Releases を解釈できません: {e}")))?;
    let current = semver::Version::parse(crate::util::LOA_VERSION).expect("version");
    let mut best: Option<(semver::Version, &Release)> = None;
    for r in releases.iter().filter(|r| !r.draft && !r.prerelease) {
        let Some(v) = r.tag_name.strip_prefix("scenarios-v") else {
            continue;
        };
        let Ok(ver) = semver::Version::parse(v) else {
            continue;
        };
        if want.is_some_and(|w| w != v) {
            continue;
        }
        if best.as_ref().is_none_or(|(b, _)| ver > *b) {
            best = Some((ver, r));
        }
    }
    let (ver, rel) =
        best.ok_or_else(|| LoaError::other("取得できるシナリオのリリースがありません"))?;
    let base = format!("loa-scenarios-{ver}.tar.gz");
    let find = |n: &str| {
        rel.assets
            .iter()
            .find(|a| a.name == n)
            .map(|a| a.browser_download_url.clone())
            .ok_or_else(|| LoaError::other(format!("リリースに {n} がありません")))
    };
    let tgz = get_bytes(&agent, &find(&base)?)?;
    let sum = String::from_utf8(get_bytes(&agent, &find(&format!("{base}.sha256"))?)?)
        .map_err(|_| LoaError::other("チェックサムが不正です"))?;
    let sig = String::from_utf8(get_bytes(&agent, &find(&format!("{base}.minisig"))?)?)
        .map_err(|_| LoaError::other("署名が不正です"))?;
    verify(&tgz, &sum, &sig, pubkey)?;
    let version = install(paths, &tgz, &current)?;
    if version != ver.to_string() {
        return Err(LoaError::other("配布物の VERSION がリリースと一致しません"));
    }
    write_atomic(&active_file(paths), version.as_bytes())?;
    Ok(version)
}

/// Checksum and signature must both match.
pub fn verify(data: &[u8], sha256_file: &str, minisig: &str, pubkey_b64: &str) -> Result<()> {
    let expected = sha256_file
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let actual = hex::encode(Sha256::digest(data));
    if expected != actual {
        return Err(LoaError::other("チェックサムが一致しないため適用しません"));
    }
    let pk = minisign_verify::PublicKey::from_base64(pubkey_b64)
        .map_err(|e| LoaError::other(format!("公開鍵が不正です: {e}")))?;
    let sig = minisign_verify::Signature::decode(minisig)
        .map_err(|e| LoaError::other(format!("署名を読めません: {e}")))?;
    pk.verify(data, &sig, false)
        .map_err(|_| LoaError::other("署名が一致しないため適用しません"))
}

/// Extract yaml files and VERSION into `scenarios/<version>/`. Rejects unsafe paths.
pub fn install(paths: &Paths, tgz: &[u8], current: &semver::Version) -> Result<String> {
    let staging = paths
        .scenarios_dir()
        .join(format!(".staging-{}", crate::util::random_hex(4)));
    crate::paths::create_private_dir(&staging)?;
    let result = (|| -> Result<String> {
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(tgz));
        let (mut files, mut total) = (0usize, 0u64);
        for entry in ar
            .entries()
            .map_err(|e| LoaError::other(format!("配布物を展開できません: {e}")))?
        {
            let mut entry =
                entry.map_err(|e| LoaError::other(format!("配布物を展開できません: {e}")))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let raw = entry
                .path()
                .map_err(|e| LoaError::other(format!("配布物のパスが不正です: {e}")))?
                .into_owned();
            // `./VERSION` (tar -C dir .) is fine; anything else but plain names is not.
            let mut path = std::path::PathBuf::new();
            for c in raw.components() {
                match c {
                    std::path::Component::CurDir => {}
                    std::path::Component::Normal(n) => path.push(n),
                    _ => return Err(LoaError::other("配布物に不正なパスが含まれています")),
                }
            }
            if path.as_os_str().is_empty() {
                continue;
            }
            let ok_ext = path.extension().and_then(|e| e.to_str()) == Some("yaml")
                || path.file_name().and_then(|n| n.to_str()) == Some("VERSION");
            if !ok_ext {
                continue;
            }
            files += 1;
            if files > MAX_FILES {
                return Err(LoaError::other("配布物のファイル数が多すぎます"));
            }
            let mut buf = Vec::new();
            (&mut entry)
                .take(MAX_FILE_BYTES + 1)
                .read_to_end(&mut buf)?;
            total += buf.len() as u64;
            if buf.len() as u64 > MAX_FILE_BYTES || total > MAX_TOTAL_BYTES {
                return Err(LoaError::other("配布物の展開後のサイズが大きすぎます"));
            }
            let dest = staging.join(&path);
            if let Some(p) = dest.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(&dest, &buf)?;
        }
        let version = std::fs::read_to_string(staging.join("VERSION"))
            .map_err(|_| LoaError::other("配布物に VERSION がありません"))?
            .trim()
            .to_string();
        semver::Version::parse(&version).map_err(|_| LoaError::other("VERSION が不正です"))?;
        check_all(&staging, current)?;
        Ok(version)
    })();
    match result {
        Ok(v) => {
            let dest = paths.scenarios_dir().join(&v);
            let _ = std::fs::remove_dir_all(&dest);
            std::fs::rename(&staging, &dest)?;
            Ok(v)
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

fn check_all(dir: &Path, _current: &semver::Version) -> Result<()> {
    fn walk(d: &Path, out: &mut Vec<std::path::PathBuf>) {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|e| e.to_str()) == Some("yaml") {
                    out.push(p);
                }
            }
        }
    }
    let mut files = Vec::new();
    walk(dir, &mut files);
    for f in files {
        let text = std::fs::read_to_string(&f)?;
        let s = crate::scenario::Scenario::from_yaml(&text)?;
        let op = crate::ops::OpKind::parse(&s.operation).ok_or_else(|| {
            LoaError::other(format!("{}: 不明な operation {}", s.id, s.operation))
        })?;
        // Version-gated scenarios are skipped at load time, so only validate compatible ones.
        let min = semver::Version::parse(&s.min_loa_version).ok();
        let cur = semver::Version::parse(crate::util::LOA_VERSION).ok();
        if min <= cur {
            crate::scenario::validate::validate(&s, op)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test vector from the minisign-verify crate documentation.
    const PK: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const SIG: &str = "untrusted comment: signature from minisign secret key\nRUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=\ntrusted comment: timestamp:1633700835\tfile:test\tprehashed\nwLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==";

    fn sum(d: &[u8]) -> String {
        format!("{}  file\n", hex::encode(Sha256::digest(d)))
    }

    #[test]
    fn valid_signature_and_checksum_pass() {
        verify(b"test", &sum(b"test"), SIG, PK).unwrap();
    }

    #[test]
    fn checksum_mismatch_rejected() {
        assert!(verify(b"test", &sum(b"other"), SIG, PK).is_err());
    }

    #[test]
    fn signature_mismatch_rejected() {
        // checksum matches the tampered data, but the signature does not
        assert!(verify(b"tampered", &sum(b"tampered"), SIG, PK).is_err());
    }

    fn tgz(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut b = tar::Builder::new(enc);
            for (name, data) in entries {
                let mut h = tar::Header::new_gnu();
                h.set_size(data.len() as u64);
                h.set_mode(0o644);
                h.set_entry_type(tar::EntryType::Regular);
                b.append_data(&mut h, name, &data[..]).unwrap();
            }
            b.into_inner().unwrap().finish().unwrap();
        }
        buf
    }

    fn scratch(tag: &str) -> (std::path::PathBuf, Paths) {
        let dir = std::env::temp_dir().join(format!("loa-bundle-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let paths = Paths::at(&dir);
        paths.ensure().unwrap();
        (dir, paths)
    }

    #[test]
    fn install_accepts_dot_slash_entries() {
        let (dir, paths) = scratch("dot");
        let yaml = b"id: session.whoami\nversion: 2\nmin_loa_version: \"0.1.0\"\noperation: session.whoami\noutputs:\n  url: { type: string }\nsteps:\n  - goto: \"https://developers.line.biz/console/\"\n  - read_url: { pattern: \"^([^?#]*)\", into: url }\n".to_vec();
        let data = tgz(&[
            ("./VERSION", b"9.9.9\n".to_vec()),
            ("./session/whoami.yaml", yaml),
        ]);
        let cur = semver::Version::parse("0.1.0").unwrap();
        assert_eq!(install(&paths, &data, &cur).unwrap(), "9.9.9");
        assert!(dir.join("scenarios/9.9.9/session/whoami.yaml").is_file());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_rejects_oversized_files_after_decompression() {
        let (dir, paths) = scratch("big");
        let big = vec![b'a'; (MAX_FILE_BYTES + 10) as usize];
        let data = tgz(&[("VERSION", b"9.9.8\n".to_vec()), ("big.yaml", big)]);
        assert!((data.len() as u64) < MAX_FILE_BYTES, "compresses well");
        let cur = semver::Version::parse("0.1.0").unwrap();
        assert!(install(&paths, &data, &cur).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn install_rejects_path_traversal() {
        let mut buf = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
            let mut b = tar::Builder::new(enc);
            let data = b"x";
            let mut h = tar::Header::new_gnu();
            h.set_size(1);
            h.set_mode(0o644);
            h.set_entry_type(tar::EntryType::Regular);
            // write a raw "../" path bypassing builder checks
            h.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../evil.yaml");
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("loa-bundle-{}", std::process::id()));
        let paths = Paths::at(&dir);
        paths.ensure().unwrap();
        let cur = semver::Version::parse("0.1.0").unwrap();
        assert!(install(&paths, &buf, &cur).is_err());
        assert!(!dir.join("evil.yaml").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
