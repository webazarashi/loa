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
    ]
    .iter()
    .map(|p| Regex::new(p).expect("valid regex"))
    .collect()
});

/// Long opaque tokens (long-lived channel access tokens, keys). Matches that start with `/`
/// are file paths or the `//host/...` part of URLs and are kept.
static OPAQUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_-]{40,}={0,2}").expect("valid regex"));

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
    let mut res = String::with_capacity(out.len());
    let mut last = 0;
    for m in OPAQUE.find_iter(&out) {
        let before = out[..m.start()].chars().next_back();
        // part of a file path or URL (`/abs/path`, `host.tld/path`, `~/x`)
        let path_like = m.as_str().starts_with('/')
            || (m.as_str().contains('/') && matches!(before, Some('.' | '/' | '\\' | '~')));
        res.push_str(&out[last..m.start()]);
        res.push_str(if path_like { m.as_str() } else { MASK });
        last = m.end();
    }
    res.push_str(&out[last..]);
    res
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
                    // `*_ref` holds a loa:// reference, never the value itself
                    let is_ref = lower.ends_with("_ref")
                        && v.as_str().is_some_and(|s| s.starts_with("loa://"));
                    if !is_ref
                        && (lower.contains("secret")
                            || lower.contains("token")
                            || lower.contains("password"))
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
    fn keeps_paths_but_masks_tokens() {
        let p = "/var/folders/ab/cdefghijklmnop_qrstuv/T/loa-e2e-flow-123/data/reports/20261119-miniapp.get";
        assert_eq!(redact(p), p);
        let u =
            "https://developers.line.biz/console/channel/1234567890/basics/very/long/path/segment";
        assert_eq!(redact(u), u);
        let tok = format!("token {}", "Ab1+".repeat(20));
        assert!(!redact(&tok).contains("Ab1+Ab1+"));
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
