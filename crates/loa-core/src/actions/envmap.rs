//! `loa env write` / `loa exec`: hand channel values to a project without printing secrets.

use super::{Ctx, liff};
use crate::error::{LoaError, Result};
use crate::resources::{ChannelHit, ChannelType};
use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;
use zeroize::Zeroizing;

#[derive(Debug, Clone, PartialEq)]
pub struct Mapping {
    pub key: String,
    pub path: String,
}

static KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").expect("regex"));

pub fn parse_map(s: &str) -> Result<Mapping> {
    let (k, p) = s
        .split_once('=')
        .ok_or_else(|| LoaError::usage(format!("--map は KEY=path の形式です: {s}")))?;
    if !KEY.is_match(k) {
        return Err(LoaError::usage(format!("環境変数名が不正です: {k}")));
    }
    Ok(Mapping {
        key: k.to_string(),
        path: p.trim().to_string(),
    })
}

/// Default mappings when none are given.
pub fn default_maps(hit: &ChannelHit) -> Vec<Mapping> {
    let mut m = vec![
        Mapping {
            key: "LINE_CHANNEL_ID".into(),
            path: "channel.id".into(),
        },
        Mapping {
            key: "LINE_CHANNEL_SECRET".into(),
            path: "channel.secret".into(),
        },
    ];
    if hit.rec.liff.len() == 1
        && let Some(name) = hit.rec.liff.keys().next()
    {
        m.push(Mapping {
            key: "LIFF_ID".into(),
            path: format!("liff.{name}.id"),
        });
    }
    if hit.rec.ty == ChannelType::Miniapp {
        m.push(Mapping {
            key: "LIFF_ID".into(),
            path: "miniapp.developing.liff_id".into(),
        });
    }
    m
}

/// Resolve every mapping to a value. Secrets stay in `Zeroizing` and are never printed.
pub fn resolve(
    ctx: &mut Ctx<'_>,
    hit: &ChannelHit,
    maps: &[Mapping],
) -> Result<Vec<(String, Zeroizing<String>)>> {
    let mut out = Vec::new();
    let mut hit = hit.clone();
    let mut liff_cache: Option<Vec<crate::line_api::LiffApp>> = None;
    for m in maps {
        let parts: Vec<&str> = m.path.split('.').collect();
        let v: String = match parts.as_slice() {
            ["channel", "id"] => hit.rec.id.clone(),
            ["channel", "name"] => hit.name.clone(),
            ["channel", "type"] => hit.rec.ty.as_str().into(),
            ["channel", "secret"] => {
                out.push((m.key.clone(), ctx.channel_secret(&hit)?));
                continue;
            }
            ["channel", "access_token"] => {
                out.push((m.key.clone(), ctx.channel_access_token(&hit)?));
                continue;
            }
            ["provider", "id"] => hit.provider_id.clone(),
            ["provider", "name"] => hit.provider.clone(),
            ["liff", name, field] => {
                if let (Some(id), &"id") = (hit.rec.liff.get(*name), field) {
                    id.clone()
                } else {
                    if liff_cache.is_none() {
                        liff_cache = Some(liff::fetch(ctx, &hit)?);
                        if let Some(b) = ctx.resources()?.business_ref(&ctx.business_id)
                            && let Some(h) = b.find_channel(&hit.key, Some(&hit.provider))?
                        {
                            hit = h;
                        }
                    }
                    let app = liff_cache
                        .as_ref()
                        .and_then(|apps| {
                            apps.iter()
                                .find(|a| a.description.as_deref() == Some(*name))
                        })
                        .ok_or_else(|| {
                            LoaError::usage(format!("LIFFアプリ {name} がありません"))
                        })?;
                    match *field {
                        "id" => app.liff_id.clone(),
                        "url" => app.view.url.clone(),
                        "liff_url" => format!("https://liff.line.me/{}", app.liff_id),
                        _ => return Err(LoaError::usage(format!("不明な値です: {}", m.path))),
                    }
                }
            }
            ["miniapp", stage, "liff_id"] => {
                hit.rec.miniapp.get(*stage).cloned().ok_or_else(|| {
                    LoaError::usage(format!("ミニアプリの {stage} のLIFF IDが未取得です"))
                        .with_next(format!("loa miniapp get {}", hit.key))
                })?
            }
            _ => {
                return Err(LoaError::usage(format!(
                    "不明な値です: {}（channel.id / channel.name / channel.secret / channel.access_token / provider.id / liff.<name>.id / liff.<name>.url / miniapp.<stage>.liff_id）",
                    m.path
                )));
            }
        };
        out.push((m.key.clone(), Zeroizing::new(v)));
    }
    Ok(out)
}

fn quote(v: &str) -> String {
    if !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./:@+=,".contains(c))
    {
        v.to_string()
    } else {
        format!(
            "\"{}\"",
            v.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    }
}

/// Update or append KEY=value lines; other lines are kept. Mode 0600.
pub fn write_env_file(path: &Path, values: &[(String, Zeroizing<String>)]) -> Result<Vec<String>> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut written = Vec::new();
    for (k, v) in values {
        let new_line = format!("{k}={}", quote(v));
        let pos = lines.iter().position(|l| {
            let t = l.trim_start();
            let t = t.strip_prefix("export ").unwrap_or(t);
            t.split_once('=')
                .map(|(key, _)| key.trim() == k)
                .unwrap_or(false)
        });
        match pos {
            Some(i) => lines[i] = new_line,
            None => lines.push(new_line),
        }
        written.push(k.clone());
    }
    let mut body = lines.join("\n");
    body.push('\n');
    let body = Zeroizing::new(body);
    crate::paths::write_atomic(path, body.as_bytes())?;
    Ok(written)
}

/// Warn when the file would be committed (inside a git work tree and not ignored).
pub fn git_unignored(path: &Path) -> bool {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(dir)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !inside {
        return false;
    }
    let ignored = std::process::Command::new("git")
        .args(["check-ignore", "-q"])
        .arg(path)
        .current_dir(dir)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    !ignored
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mappings() {
        assert_eq!(
            parse_map("LIFF_ID=liff.main.id").unwrap().path,
            "liff.main.id"
        );
        assert!(parse_map("1BAD=x").is_err());
        assert!(parse_map("NOEQ").is_err());
    }

    #[test]
    fn env_file_update_keeps_other_lines() {
        let p = std::env::temp_dir().join(format!("loa-env-{}", std::process::id()));
        std::fs::write(&p, "# comment\nKEEP=1\nexport LINE_CHANNEL_ID=old\n").unwrap();
        let vals = vec![
            (
                "LINE_CHANNEL_ID".to_string(),
                Zeroizing::new("2001".to_string()),
            ),
            (
                "LINE_CHANNEL_SECRET".to_string(),
                Zeroizing::new("a b".to_string()),
            ),
        ];
        let written = write_env_file(&p, &vals).unwrap();
        assert_eq!(written, vec!["LINE_CHANNEL_ID", "LINE_CHANNEL_SECRET"]);
        let s = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            s,
            "# comment\nKEEP=1\nLINE_CHANNEL_ID=2001\nLINE_CHANNEL_SECRET=\"a b\"\n"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(p);
    }
}
