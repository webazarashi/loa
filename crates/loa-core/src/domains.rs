//! Domain allowlist enforced by loa (spec 6-3). agent-browser's own `--allowed-domains`
//! cannot be combined with persistent login state, so loa checks every navigation itself.

use crate::error::{Code, LoaError, Result};

pub const LINE_HOSTS: [&str; 5] = [
    "developers.line.biz",
    "manager.line.biz",
    "entry.line.biz",
    "account.line.biz",
    "access.line.me",
];

/// Hosts where a person signs in. Automation pauses here and waits for the human.
pub const LOGIN_HOSTS: [&str; 2] = ["account.line.biz", "access.line.me"];

#[derive(Debug, Clone)]
pub struct Allowlist {
    hosts: Vec<String>,
}

impl Allowlist {
    /// Production list. In test-hooks builds the list comes from `LOA_TEST_ALLOW_HOSTS`
    /// and real LINE hosts are refused, so tests can never touch the real service.
    pub fn default_for_build() -> Allowlist {
        #[cfg(feature = "test-hooks")]
        {
            let hosts = std::env::var("LOA_TEST_ALLOW_HOSTS")
                .unwrap_or_else(|_| "127.0.0.1,localhost".into())
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && !s.ends_with("line.biz") && !s.ends_with("line.me"))
                .collect();
            Allowlist { hosts }
        }
        #[cfg(not(feature = "test-hooks"))]
        {
            Allowlist {
                hosts: LINE_HOSTS.iter().map(|s| s.to_string()).collect(),
            }
        }
    }

    pub fn new(hosts: &[&str]) -> Allowlist {
        Allowlist {
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn host_allowed(&self, host: &str) -> bool {
        self.hosts.iter().any(|h| h.eq_ignore_ascii_case(host))
    }

    /// Error unless the URL is http(s) on an allowed host (about:blank is tolerated).
    pub fn check(&self, raw: &str) -> Result<()> {
        if raw.is_empty() || raw == "about:blank" {
            return Ok(());
        }
        let parsed = url::Url::parse(raw).map_err(|_| blocked(raw))?;
        let scheme_ok = parsed.scheme() == "https"
            || (parsed.scheme() == "http" && cfg!(feature = "test-hooks"));
        match parsed.host_str() {
            Some(h) if scheme_ok && self.host_allowed(h) => Ok(()),
            _ => Err(blocked(raw)),
        }
    }
}

fn blocked(raw: &str) -> LoaError {
    LoaError::new(
        Code::DomainBlocked,
        format!(
            "許可されていない移動先です: {}",
            crate::redact::redact_url(raw)
        ),
    )
}

/// Test builds only: map real LINE hosts to the local fake console
/// (`LOA_TEST_HOST_MAP=developers.line.biz=http://127.0.0.1:9/dev,...`). No-op otherwise.
pub fn test_rewrite(u: &str) -> String {
    #[cfg(feature = "test-hooks")]
    if let Ok(map) = std::env::var("LOA_TEST_HOST_MAP") {
        for pair in map.split(',') {
            if let Some((host, base)) = pair.split_once('=') {
                let prefix = format!("https://{}", host.trim());
                if let Some(rest) = u.strip_prefix(&prefix) {
                    return format!("{}{}", base.trim().trim_end_matches('/'), rest);
                }
            }
        }
    }
    u.to_string()
}

/// Is this a sign-in page (LINE Business ID / LINE Login)?
pub fn is_login_url(raw: &str) -> bool {
    match url::Url::parse(raw) {
        Ok(u) => {
            let host = u.host_str().unwrap_or("");
            LOGIN_HOSTS.iter().any(|h| h.eq_ignore_ascii_case(host)) || u.path().contains("/login")
        }
        Err(_) => false,
    }
}

/// Minimal glob for URL waits: `**` = anything, `*` = anything except `/`.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    let mut re = String::from("^");
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    re.push_str(".*");
                } else {
                    re.push_str("[^/]*");
                }
            }
            '?' => re.push('.'),
            other => re.push_str(&regex::escape(&other.to_string())),
        }
    }
    re.push('$');
    regex::Regex::new(&re)
        .map(|r| r.is_match(text))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_checks_scheme_and_host() {
        let a = Allowlist::new(&["developers.line.biz"]);
        assert!(a.check("https://developers.line.biz/console/").is_ok());
        assert!(a.check("https://evil.example/").is_err());
        assert!(
            a.check("https://developers.line.biz.evil.example/")
                .is_err()
        );
        assert!(a.check("javascript:alert(1)").is_err());
        assert!(a.check("about:blank").is_ok());
        assert_eq!(
            a.check("https://x.example/").unwrap_err().code,
            Code::DomainBlocked
        );
    }

    #[test]
    fn login_detection() {
        assert!(is_login_url("https://account.line.biz/login?redirectUri=x"));
        assert!(is_login_url("https://access.line.me/oauth2/v2.1/login"));
        assert!(!is_login_url("https://developers.line.biz/console/"));
    }

    #[test]
    fn globs() {
        assert!(glob_match(
            "**/channel/*/basics",
            "https://developers.line.biz/console/channel/123/basics"
        ));
        assert!(!glob_match(
            "**/channel/*/basics",
            "https://developers.line.biz/console/channel/123/x/basics"
        ));
        assert!(glob_match("https://h/*", "https://h/abc"));
    }
}
