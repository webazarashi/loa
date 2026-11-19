//! Browser driver. loa talks to agent-browser as a separate process (spec 6). Every call is a
//! `batch --bail --json` with the commands on stdin, so typed values never appear in argv.

pub mod snapshot;

use crate::error::{Code, LoaError, Result};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use zeroize::Zeroizing;

pub use snapshot::{Node, Snapshot};

/// agent-browser versions loa is tested against.
pub const SUPPORTED_AGENT_BROWSER: &str = ">=0.41.0, <0.42.0";
pub const NAMESPACE: &str = "loa";

#[derive(Debug, Clone, PartialEq)]
pub enum BrowserErrorKind {
    NotFound,
    Timeout,
    Launch,
    Other,
}

#[derive(Debug, Clone)]
pub struct BrowserError {
    pub kind: BrowserErrorKind,
    pub index: Option<usize>,
    pub message: String,
}

impl BrowserError {
    pub fn other(m: impl Into<String>) -> Self {
        BrowserError {
            kind: BrowserErrorKind::Other,
            index: None,
            message: m.into(),
        }
    }
    fn classify(message: String, index: Option<usize>) -> Self {
        let lower = message.to_lowercase();
        let kind = if lower.contains("not found")
            || lower.contains("none match")
            || lower.contains("no element")
            || lower.contains("elements have role")
        {
            BrowserErrorKind::NotFound
        } else if lower.contains("timed out") || lower.contains("timeout") {
            BrowserErrorKind::Timeout
        } else if lower.contains("launch") || lower.contains("daemon") {
            BrowserErrorKind::Launch
        } else {
            BrowserErrorKind::Other
        };
        BrowserError {
            kind,
            index,
            message: crate::redact::redact(&message),
        }
    }
}

impl std::fmt::Display for BrowserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub type BResult<T> = std::result::Result<T, BrowserError>;

/// One agent-browser command as argv words (without global flags).
pub type Cmd = Vec<String>;

pub fn cmd(words: &[&str]) -> Cmd {
    words.iter().map(|s| s.to_string()).collect()
}

pub trait Browser {
    /// Run commands in order, stopping at the first failure. Returns each command's `result`.
    fn batch(&mut self, cmds: Vec<Cmd>) -> BResult<Vec<Value>>;

    fn run(&mut self, c: Cmd) -> BResult<Value> {
        Ok(self
            .batch(vec![c])?
            .into_iter()
            .next()
            .unwrap_or(Value::Null))
    }

    fn url(&mut self) -> BResult<String> {
        let v = self.run(cmd(&["get", "url"]))?;
        Ok(v.get("url")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string())
    }

    fn snapshot(&mut self) -> BResult<Snapshot> {
        let v = self.run(cmd(&["snapshot"]))?;
        Ok(Snapshot::parse(
            v.get("snapshot").and_then(Value::as_str).unwrap_or(""),
        ))
    }

    fn screenshot(&mut self, path: &Path) -> BResult<()> {
        self.run(vec!["screenshot".into(), path.display().to_string()])
            .map(|_| ())
    }

    fn close(&mut self) -> BResult<()>;

    /// Drop saved login state for this session.
    fn forget_state(&mut self) -> BResult<()>;
}

#[derive(Clone)]
pub struct AgentBrowserConfig {
    pub bin: PathBuf,
    pub session: String,
    pub namespace: String,
    pub headed: bool,
    pub encryption_key: Zeroizing<String>,
    pub config_file: PathBuf,
    pub cwd: PathBuf,
    pub idle_timeout: String,
    pub default_timeout_ms: u64,
}

pub struct AgentBrowser {
    cfg: AgentBrowserConfig,
}

impl AgentBrowser {
    pub fn new(cfg: AgentBrowserConfig) -> Self {
        AgentBrowser { cfg }
    }

    fn base_command(&self) -> Command {
        let mut c = Command::new(&self.cfg.bin);
        c.current_dir(&self.cfg.cwd);
        // Never inherit agent-browser settings (init scripts, proxies, policies) from the caller.
        for (k, _) in std::env::vars_os() {
            if k.to_string_lossy().starts_with("AGENT_BROWSER_") {
                c.env_remove(&k);
            }
        }
        c.env(
            "AGENT_BROWSER_ENCRYPTION_KEY",
            self.cfg.encryption_key.as_str(),
        );
        c.env("AGENT_BROWSER_STATE_EXPIRE_DAYS", "180");
        c.env(
            "AGENT_BROWSER_DEFAULT_TIMEOUT",
            self.cfg.default_timeout_ms.to_string(),
        );
        c.args([
            "--config",
            &self.cfg.config_file.display().to_string(),
            "--namespace",
            &self.cfg.namespace,
            "--session",
            &self.cfg.session,
            "--restore",
            "--idle-timeout",
            &self.cfg.idle_timeout,
            "--args",
            "--lang=ja",
        ]);
        if self.cfg.headed {
            c.arg("--headed");
        }
        c
    }
}

impl Browser for AgentBrowser {
    fn batch(&mut self, cmds: Vec<Cmd>) -> BResult<Vec<Value>> {
        if cmds.is_empty() {
            return Ok(Vec::new());
        }
        let payload = Zeroizing::new(
            serde_json::to_string(&cmds).map_err(|e| BrowserError::other(e.to_string()))?,
        );
        let mut c = self.base_command();
        c.args(["batch", "--bail", "--json", "--omit-command"]);
        c.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| BrowserError {
            kind: BrowserErrorKind::Launch,
            index: None,
            message: format!("agent-browser を起動できません: {e}"),
        })?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(payload.as_bytes())
                .map_err(|e| BrowserError::other(e.to_string()))?;
        }
        let out = child
            .wait_with_output()
            .map_err(|e| BrowserError::other(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        parse_batch_output(&stdout, &String::from_utf8_lossy(&out.stderr), cmds.len())
    }

    fn close(&mut self) -> BResult<()> {
        let mut c = self.base_command();
        c.args(["close"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = c.status();
        Ok(())
    }

    fn forget_state(&mut self) -> BResult<()> {
        self.close()?;
        let mut c = self.base_command();
        c.args(["state", "clear", &self.cfg.session])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let _ = c.status();
        Ok(())
    }
}

/// Parse `batch --json --omit-command` output.
pub fn parse_batch_output(stdout: &str, stderr: &str, expected: usize) -> BResult<Vec<Value>> {
    let trimmed = stdout.trim();
    let parsed: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => {
            let msg = if trimmed.is_empty() {
                stderr.trim()
            } else {
                trimmed
            };
            return Err(BrowserError::classify(
                format!(
                    "agent-browser の出力を解釈できません: {}",
                    truncate(msg, 400)
                ),
                None,
            ));
        }
    };
    match parsed {
        Value::Array(items) => {
            let mut results = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                let ok = item
                    .get("success")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if !ok {
                    let msg = item
                        .get("error")
                        .and_then(Value::as_str)
                        .unwrap_or("不明なエラー")
                        .to_string();
                    let idx = item
                        .get("index")
                        .and_then(Value::as_u64)
                        .map(|n| n as usize)
                        .unwrap_or(i);
                    return Err(BrowserError::classify(msg, Some(idx)));
                }
                results.push(item.get("result").cloned().unwrap_or(Value::Null));
            }
            if results.len() < expected {
                return Err(BrowserError::other(format!(
                    "agent-browser が {} 件中 {} 件しか実行しませんでした",
                    expected,
                    results.len()
                )));
            }
            Ok(results)
        }
        Value::Object(o) => {
            let msg = o
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("不明なエラー")
                .to_string();
            Err(BrowserError::classify(msg, None))
        }
        _ => Err(BrowserError::other("agent-browser の出力が不正です")),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// Find the agent-browser executable. On Windows npm installs `.cmd` shims, which
/// `Command::new` cannot run, so the native `.exe` next to the package is used.
pub fn resolve_binary(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        return Err(LoaError::setup(format!(
            "config.toml の agent_browser_path ({}) が見つかりません",
            p.display()
        )));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if cfg!(windows) {
            let exe = dir.join("agent-browser.exe");
            if exe.is_file() {
                return Ok(exe);
            }
            if dir.join("agent-browser.cmd").is_file() {
                let native = dir
                    .join("node_modules")
                    .join("agent-browser")
                    .join("bin")
                    .join("agent-browser-win32-x64.exe");
                if native.is_file() {
                    return Ok(native);
                }
            }
        } else {
            let p = dir.join("agent-browser");
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(LoaError::setup("agent-browser が見つかりません").with_hint(
        "npm install -g agent-browser@0.41 または brew install agent-browser で導入し、loa setup を実行してください",
    ))
}

/// `agent-browser --version` → semver, checked against [`SUPPORTED_AGENT_BROWSER`].
pub fn check_version(bin: &Path) -> Result<semver::Version> {
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .map_err(|e| LoaError::setup(format!("agent-browser を実行できません: {e}")))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let ver = text
        .split_whitespace()
        .find_map(|w| semver::Version::parse(w.trim_start_matches('v')).ok())
        .ok_or_else(|| {
            LoaError::setup(format!("agent-browser のバージョンを読めません: {text}"))
        })?;
    let req = semver::VersionReq::parse(SUPPORTED_AGENT_BROWSER).expect("valid req");
    if !req.matches(&ver) {
        return Err(LoaError::new(
            Code::SetupRequired,
            format!("agent-browser {ver} には対応していません（対応: {SUPPORTED_AGENT_BROWSER}）"),
        )
        .with_hint("npm install -g agent-browser@0.41 で対応バージョンを入れてください")
        .with_next("loa setup"));
    }
    Ok(ver)
}

/// Contents of the loa-owned agent-browser config (replaces auto-discovered project configs).
pub fn default_config_json() -> String {
    "{\n  \"headed\": true\n}\n".to_string()
}

/// Translate a browser failure into a loa error.
pub fn to_loa_error(e: BrowserError) -> LoaError {
    match e.kind {
        BrowserErrorKind::NotFound | BrowserErrorKind::Timeout => {
            LoaError::ui_changed(format!("画面の要素が見つかりません: {}", e.message))
        }
        BrowserErrorKind::Launch => {
            LoaError::setup(format!("ブラウザを起動できません: {}", e.message))
        }
        BrowserErrorKind::Other => {
            LoaError::other(format!("ブラウザ操作に失敗しました: {}", e.message))
        }
    }
}

/// Scripted fake browser for unit tests.
#[derive(Default)]
pub struct FakeBrowser {
    pub calls: Vec<Cmd>,
    /// Responses popped for each command; missing → `{}`.
    pub responses: std::collections::VecDeque<BResult<Value>>,
    pub current_url: String,
}

impl Browser for FakeBrowser {
    fn batch(&mut self, cmds: Vec<Cmd>) -> BResult<Vec<Value>> {
        let mut out = Vec::new();
        for c in cmds {
            self.calls.push(c.clone());
            if c.first().map(String::as_str) == Some("get")
                && c.get(1).map(String::as_str) == Some("url")
            {
                out.push(serde_json::json!({"url": self.current_url}));
                continue;
            }
            match self.responses.pop_front() {
                Some(Ok(v)) => out.push(v),
                Some(Err(e)) => return Err(e),
                None => out.push(serde_json::json!({})),
            }
        }
        Ok(out)
    }
    fn close(&mut self) -> BResult<()> {
        Ok(())
    }
    fn forget_state(&mut self) -> BResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_success_array() {
        let out = r#"[{"error":null,"index":0,"result":{"url":"https://x/"},"success":true}]"#;
        let r = parse_batch_output(out, "", 1).unwrap();
        assert_eq!(r[0]["url"], "https://x/");
    }

    #[test]
    fn parses_failure_with_index() {
        let out = r#"[{"error":null,"index":0,"result":{},"success":true},{"error":"3 elements have role \"button\", but none match name \"x\"","index":1,"result":null,"success":false}]"#;
        let e = parse_batch_output(out, "", 2).unwrap_err();
        assert_eq!(e.index, Some(1));
        assert_eq!(e.kind, BrowserErrorKind::NotFound);
    }

    #[test]
    fn parses_top_level_error() {
        let e = parse_batch_output(
            r#"{"success":false,"data":null,"error":"Wait timed out after 1500ms"}"#,
            "",
            1,
        )
        .unwrap_err();
        assert_eq!(e.kind, BrowserErrorKind::Timeout);
    }

    #[test]
    fn short_output_is_error() {
        let out = r#"[{"error":null,"index":0,"result":{},"success":true}]"#;
        assert!(parse_batch_output(out, "", 2).is_err());
    }
}
