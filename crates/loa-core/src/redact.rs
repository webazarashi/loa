//! Masking of secrets in anything that is logged, printed or written to a report.

use regex::Regex;
use std::sync::{LazyLock, Mutex};

pub const MASK: &str = "********";

static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        // JWT / JWS (channel access tokens v2.1, stateless tokens)
        r"eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        // Bearer headers
        r"(?i)bearer\s+[A-Za-z0-9._~+/=-]{16,}",
        // Channel secrets are 32 hex characters
        r"\b[0-9a-f]{32}\b",
        // Long opaque tokens (long-lived channel access tokens, keys)
        r"[A-Za-z0-9+/_-]{40,}={0,2}",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("valid regex"))
    .collect()
});

/// Values that were read as secrets during this process. They are masked wherever they appear.
static KNOWN: LazyLock<Mutex<Vec<String>>> = LazyLock::new(|| Mutex::new(Vec::new()));

pub fn register_secret(value: &str) {
    if value.len() < 6 {
        return;
    }
    if let Ok(mut k) = KNOWN.lock()
        && !k.iter().any(|v| v == value)
    {
        k.push(value.to_string());
    }
}

pub fn redact(input: &str) -> String {
    let mut out = input.to_string();
    if let Ok(k) = KNOWN.lock() {
        for v in k.iter() {
            out = out.replace(v.as_str(), MASK);
        }
    }
    for re in PATTERNS.iter() {
        out = re.replace_all(&out, MASK).into_owned();
    }
    out
}

/// Strip query string and fragment from a URL (they can carry codes and tokens).
pub fn redact_url(u: &str) -> String {
    match url::Url::parse(u) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            redact(parsed.as_str())
        }
        Err(_) => redact(u.split(['?', '#']).next().unwrap_or("")),
    }
}

pub fn redact_json(v: &serde_json::Value) -> serde_json::Value {
    use serde_json::Value;
    match v {
        Value::String(s) => Value::String(redact(s)),
        Value::Array(a) => Value::Array(a.iter().map(redact_json).collect()),
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, v)| {
                    let lower = k.to_ascii_lowercase();
                    if lower.contains("secret")
                        || lower.contains("token")
                        || lower.contains("password")
                    {
                        (k.clone(), Value::String(MASK.into()))
                    } else {
                        (k.clone(), redact_json(v))
                    }
                })
                .collect(),
        ),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_channel_secret_and_jwt() {
        let s = "secret=0123456789abcdef0123456789abcdef token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijklmnop";
        let r = redact(s);
        assert!(!r.contains("0123456789abcdef0123456789abcdef"));
        assert!(!r.contains("eyJhbGci"));
    }

    #[test]
    fn keeps_channel_ids() {
        assert_eq!(redact("channel 2001234567"), "channel 2001234567");
    }

    #[test]
    fn masks_registered_values() {
        register_secret("short-but-registered");
        assert_eq!(redact("x short-but-registered y"), format!("x {MASK} y"));
    }

    #[test]
    fn url_query_and_fragment_removed() {
        assert_eq!(
            redact_url("https://developers.line.biz/console/?code=abc#token=x"),
            "https://developers.line.biz/console/"
        );
    }

    #[test]
    fn json_secret_keys_masked() {
        let v = serde_json::json!({"channel_secret": "x", "name": "a"});
        let r = redact_json(&v);
        assert_eq!(r["channel_secret"], MASK);
        assert_eq!(r["name"], "a");
    }
}
