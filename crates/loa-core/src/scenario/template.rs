//! `{{var}}` substitution. Unknown variables are errors; URL substitution percent-encodes values.

use crate::error::{LoaError, Result};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::LazyLock;

static VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{\s*([A-Za-z_][A-Za-z0-9_]*)\s*\}\}").expect("regex"));

pub type Vars = BTreeMap<String, String>;

pub fn variables(template: &str) -> Vec<String> {
    VAR.captures_iter(template)
        .map(|c| c[1].to_string())
        .collect()
}

pub fn render(template: &str, vars: &Vars) -> Result<String> {
    render_with(template, vars, |v| v.to_string())
}

/// For URLs: values are percent-encoded so they cannot change host or path structure.
pub fn render_url(template: &str, vars: &Vars) -> Result<String> {
    render_with(template, vars, encode_component)
}

fn render_with(template: &str, vars: &Vars, enc: impl Fn(&str) -> String) -> Result<String> {
    let mut missing = None;
    let out = VAR.replace_all(template, |c: &regex::Captures| match vars.get(&c[1]) {
        Some(v) => enc(v),
        None => {
            missing.get_or_insert_with(|| c[1].to_string());
            String::new()
        }
    });
    match missing {
        Some(m) => Err(LoaError::other(format!(
            "シナリオの変数 {{{{{m}}}}} に値がありません"
        ))),
        None => Ok(out.into_owned()),
    }
}

pub fn encode_component(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            // `@` is a valid path character (official account IDs: /account/@abc1234)
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'@' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        [
            ("name".to_string(), "a b/../c".to_string()),
            ("id".to_string(), "123".to_string()),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn renders_and_encodes() {
        assert_eq!(
            render("x {{name}} {{ id }}", &vars()).unwrap(),
            "x a b/../c 123"
        );
        assert_eq!(
            render_url("https://h/p/{{name}}", &vars()).unwrap(),
            "https://h/p/a%20b%2F..%2Fc"
        );
    }

    #[test]
    fn keeps_at_sign_in_paths() {
        let v: Vars = [("oa".to_string(), "@abc1234".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            render_url("https://manager.line.biz/account/{{oa}}/setting", &v).unwrap(),
            "https://manager.line.biz/account/@abc1234/setting"
        );
    }

    #[test]
    fn missing_variable_is_error() {
        assert!(render("{{nope}}", &vars()).is_err());
    }

    #[test]
    fn lists_variables() {
        assert_eq!(variables("{{a}}-{{ b }}"), vec!["a", "b"]);
    }
}
