//! Success envelope (`{ok:true,...}`) and rendering for humans / JSON.

use crate::error::LoaError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Envelope {
    pub ok: bool,
    pub resource: String,
    pub action: String,
    #[serde(default)]
    pub data: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl Envelope {
    pub fn new(resource: impl Into<String>, action: impl Into<String>, data: Value) -> Self {
        Envelope {
            ok: true,
            resource: resource.into(),
            action: action.into(),
            data,
            next: Vec::new(),
            warnings: Vec::new(),
        }
    }

    pub fn with_next(mut self, next: impl Into<String>) -> Self {
        self.next.push(next.into());
        self
    }

    pub fn with_warning(mut self, w: impl Into<String>) -> Self {
        self.warnings.push(w.into());
        self
    }
}

/// Final outcome of a command: success envelope or error.
pub type Outcome = std::result::Result<Envelope, LoaError>;

#[derive(Serialize)]
struct ErrorJson<'a> {
    ok: bool,
    #[serde(flatten)]
    err: &'a LoaError,
}

pub fn error_json(err: &LoaError) -> String {
    let mut e = err.clone();
    if e.hint.is_none() {
        e.hint = Some(e.code.default_hint().to_string());
    }
    serde_json::to_string(&ErrorJson { ok: false, err: &e }).unwrap_or_else(|_| {
        format!(
            "{{\"ok\":false,\"code\":\"{}\",\"message\":\"serialization failed\"}}",
            err.code
        )
    })
}

pub fn envelope_json(env: &Envelope) -> String {
    serde_json::to_string(env).unwrap_or_else(|_| "{\"ok\":true}".into())
}

/// Human readable rendering of a successful result.
pub fn render_human(env: &Envelope) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "✓ {} {}\n",
        env.resource,
        action_label(&env.action)
    ));
    render_value(&env.data, 1, &mut out);
    for w in &env.warnings {
        out.push_str(&format!("! {w}\n"));
    }
    if !env.next.is_empty() {
        out.push_str("次のコマンド:\n");
        for n in &env.next {
            out.push_str(&format!("  {n}\n"));
        }
    }
    out
}

pub fn render_human_error(err: &LoaError) -> String {
    let mut out = format!("✗ {} {}\n", err.code, err.message);
    out.push_str(&format!("  {}\n", err.hint_or_default()));
    if let Some(t) = &err.task {
        out.push_str(&format!("  タスク: {t}\n"));
    }
    if let Some(r) = &err.report {
        out.push_str(&format!("  記録: {r}\n"));
    }
    if !err.next.is_empty() {
        out.push_str("次のコマンド:\n");
        for n in &err.next {
            out.push_str(&format!("  {n}\n"));
        }
    }
    out
}

fn action_label(a: &str) -> &str {
    match a {
        "created" => "作成しました",
        "updated" => "更新しました",
        "deleted" => "削除しました",
        "unchanged" => "変更はありません",
        "listed" => "一覧",
        "got" => "取得しました",
        "planned" => "(変更せずに確認しました)",
        other => other,
    }
}

fn render_value(v: &Value, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    match v {
        Value::Null => {}
        Value::Object(m) => {
            for (k, val) in m {
                match val {
                    Value::Object(_) | Value::Array(_) => {
                        out.push_str(&format!("{pad}{k}:\n"));
                        render_value(val, depth + 1, out);
                    }
                    _ => out.push_str(&format!("{pad}{k}: {}\n", scalar(val))),
                }
            }
        }
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str(&format!("{pad}(なし)\n"));
            }
            for item in items {
                match item {
                    Value::Object(m) => {
                        let line: Vec<String> = m
                            .iter()
                            .map(|(k, v)| format!("{k}={}", scalar(v)))
                            .collect();
                        out.push_str(&format!("{pad}- {}\n", line.join("  ")));
                    }
                    _ => out.push_str(&format!("{pad}- {}\n", scalar(item))),
                }
            }
        }
        other => out.push_str(&format!("{pad}{}\n", scalar(other))),
    }
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "-".into(),
        Value::Array(a) => format!("[{} 件]", a.len()),
        Value::Object(_) => "{…}".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Code;
    use serde_json::json;

    #[test]
    fn error_json_has_ok_false_code_and_hint() {
        let e = LoaError::new(Code::LoginRequired, "x").with_task("t_1");
        let v: Value = serde_json::from_str(&error_json(&e)).unwrap();
        assert_eq!(v["ok"], json!(false));
        assert_eq!(v["code"], json!("LOGIN_REQUIRED"));
        assert_eq!(v["task"], json!("t_1"));
        assert!(v["hint"].is_string());
    }

    #[test]
    fn envelope_json_matches_spec_shape() {
        let env = Envelope::new("channel", "created", json!({"id": "1"})).with_next("loa x");
        let v: Value = serde_json::from_str(&envelope_json(&env)).unwrap();
        assert_eq!(v["ok"], json!(true));
        assert_eq!(v["resource"], json!("channel"));
        assert_eq!(v["next"][0], json!("loa x"));
    }
}
