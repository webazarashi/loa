//! Scenarios: YAML procedures for screens that have no API (spec 7).

pub mod model;
pub mod runner;
pub mod template;
pub mod validate;

pub use model::{Scenario, Status};
pub use runner::{Outputs, Runner};
pub use template::Vars;

use crate::error::{LoaError, Result};
use crate::paths::Paths;
use include_dir::{Dir, include_dir};
use std::collections::BTreeMap;
use std::path::PathBuf;

static EMBEDDED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../scenarios");

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Embedded,
    Downloaded(String),
    Override(PathBuf),
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Embedded => "同梱".into(),
            Source::Downloaded(v) => format!("取得済み {v}"),
            Source::Override(p) => format!("ローカル上書き {}", p.display()),
        }
    }
}

pub struct Catalog {
    items: BTreeMap<String, (Scenario, Source)>,
    pub warnings: Vec<String>,
    pub bundle_version: String,
}

pub fn embedded_version() -> String {
    EMBEDDED
        .get_file("VERSION")
        .and_then(|f| f.contents_utf8())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "0.0.0".into())
}

fn embedded_files() -> Vec<(String, String)> {
    let mut out = Vec::new();
    fn walk(d: &Dir<'_>, out: &mut Vec<(String, String)>) {
        for f in d.files() {
            if f.path().extension().and_then(|e| e.to_str()) == Some("yaml")
                && let Some(s) = f.contents_utf8()
            {
                out.push((f.path().display().to_string(), s.to_string()));
            }
        }
        for sub in d.dirs() {
            walk(sub, out);
        }
    }
    walk(&EMBEDDED, &mut out);
    out
}

fn dir_files(dir: &std::path::Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    fn walk(d: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().and_then(|e| e.to_str()) == Some("yaml")
                && let Ok(s) = std::fs::read_to_string(&p)
            {
                out.push((p.display().to_string(), s));
            }
        }
    }
    walk(dir, &mut out);
    out
}

impl Catalog {
    /// Precedence: local override dir (with a warning) > downloaded bundle > embedded.
    pub fn load(paths: &Paths, pin: Option<&str>) -> Result<Catalog> {
        let mut items: BTreeMap<String, (Scenario, Source)> = BTreeMap::new();
        let mut warnings = Vec::new();
        let current = semver::Version::parse(crate::util::LOA_VERSION).expect("version");
        let mut add = |files: Vec<(String, String)>, src: Source, warnings: &mut Vec<String>| {
            for (path, text) in files {
                match Scenario::from_yaml(&text) {
                    Ok(s) => {
                        let too_new = semver::Version::parse(&s.min_loa_version)
                            .map(|v| v > current)
                            .unwrap_or(true);
                        if too_new {
                            warnings.push(format!(
                                "{} は loa {} 以上が必要なため使いません",
                                s.id, s.min_loa_version
                            ));
                            continue;
                        }
                        items.insert(s.id.clone(), (s, src.clone()));
                    }
                    Err(e) => warnings.push(format!("{path}: {}", e.message)),
                }
            }
        };
        add(embedded_files(), Source::Embedded, &mut warnings);
        let mut bundle_version = embedded_version();
        if let Some(v) = crate::bundle::active_version(paths, pin) {
            let dir = paths.scenarios_dir().join(&v);
            add(
                dir_files(&dir),
                Source::Downloaded(v.clone()),
                &mut warnings,
            );
            bundle_version = v;
        }
        if let Ok(dir) = std::env::var("LOA_SCENARIOS_DIR") {
            let dir = PathBuf::from(dir);
            warnings.push(format!(
                "開発用ディレクトリ {} のシナリオを使用しています（公式配布物ではありません）",
                dir.display()
            ));
            add(
                dir_files(&dir),
                Source::Override(dir.clone()),
                &mut warnings,
            );
        }
        Ok(Catalog {
            items,
            warnings,
            bundle_version,
        })
    }

    pub fn get(&self, id: &str) -> Result<&Scenario> {
        self.items.get(id).map(|(s, _)| s).ok_or_else(|| {
            LoaError::setup(format!("シナリオ {id} がありません"))
                .with_hint("loa scenarios update で最新のシナリオを取得してください")
                .with_next("loa scenarios update")
        })
    }

    pub fn source(&self, id: &str) -> Option<&Source> {
        self.items.get(id).map(|(_, s)| s)
    }

    pub fn list(&self) -> impl Iterator<Item = (&Scenario, &Source)> {
        self.items.values().map(|(s, src)| (s, src))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::OpKind;

    /// Every bundled scenario parses and passes validation for the operation it declares.
    #[test]
    fn bundled_scenarios_are_valid() {
        let files = embedded_files();
        assert!(!files.is_empty(), "no bundled scenarios");
        for (path, text) in files {
            let s = Scenario::from_yaml(&text).unwrap_or_else(|e| panic!("{path}: {}", e.message));
            let op = OpKind::parse(&s.operation)
                .unwrap_or_else(|| panic!("{path}: unknown op {}", s.operation));
            validate::validate(&s, op).unwrap_or_else(|e| panic!("{path}: {}", e.message));
        }
    }
}
