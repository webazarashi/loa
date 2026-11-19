//! LINE APIs used instead of the browser where they exist (spec 2): LIFF Server API and the
//! Messaging API webhook endpoints. Tokens are stateless (15 min), issued per use, never stored.

use crate::error::{Code, LoaError, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeroize::Zeroizing;

pub const API_BASE: &str = "https://api.line.me";

pub struct LineApi {
    base: String,
    agent: ureq::Agent,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiffView {
    #[serde(rename = "type")]
    pub ty: String,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module_mode: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LiffApp {
    pub liff_id: String,
    pub view: LiffView,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub permanent_link_pattern: Option<String>,
    #[serde(default)]
    pub scope: Option<Vec<String>>,
    #[serde(default)]
    pub bot_prompt: Option<String>,
}

/// Fields for add / update. `None` = leave unchanged on update.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct LiffInput {
    pub name: Option<String>,
    pub url: Option<String>,
    /// compact | tall | full
    pub size: Option<String>,
    pub scope: Option<Vec<String>>,
    /// normal | aggressive | none
    pub bot_prompt: Option<String>,
}

impl LiffInput {
    fn body(&self, for_add: bool) -> Value {
        let mut m = serde_json::Map::new();
        if self.url.is_some() || self.size.is_some() || for_add {
            let mut view = serde_json::Map::new();
            if let Some(s) = &self.size {
                view.insert("type".into(), Value::String(s.clone()));
            } else if for_add {
                view.insert("type".into(), Value::String("full".into()));
            }
            if let Some(u) = &self.url {
                view.insert("url".into(), Value::String(u.clone()));
            }
            m.insert("view".into(), Value::Object(view));
        }
        if let Some(n) = &self.name {
            m.insert("description".into(), Value::String(n.clone()));
        }
        if let Some(s) = &self.scope {
            m.insert("scope".into(), serde_json::json!(s));
        } else if for_add {
            m.insert("scope".into(), serde_json::json!(["openid", "profile"]));
        }
        if let Some(b) = &self.bot_prompt {
            m.insert("botPrompt".into(), Value::String(b.clone()));
        }
        if for_add {
            m.insert(
                "permanentLinkPattern".into(),
                Value::String("concat".into()),
            );
        }
        Value::Object(m)
    }

    pub fn validate(&self, for_add: bool) -> Result<()> {
        if for_add && (self.url.is_none() || self.name.is_none()) {
            return Err(LoaError::usage(
                "LIFFアプリの追加には --name と --url が必要です",
            ));
        }
        if let Some(u) = &self.url
            && !u.starts_with("https://")
        {
            return Err(LoaError::usage(
                "LIFFのエンドポイントURLは https:// で始めてください",
            ));
        }
        if let Some(s) = &self.size
            && !matches!(s.as_str(), "compact" | "tall" | "full")
        {
            return Err(LoaError::usage(
                "--size は compact / tall / full のいずれかです",
            ));
        }
        if let Some(b) = &self.bot_prompt
            && !matches!(b.as_str(), "normal" | "aggressive" | "none")
        {
            return Err(LoaError::usage(
                "--bot-prompt は normal / aggressive / none のいずれかです",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct WebhookInfo {
    pub endpoint: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebhookTestResult {
    pub success: bool,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

impl LineApi {
    pub fn new() -> Result<LineApi> {
        #[cfg(feature = "test-hooks")]
        let base = std::env::var("LOA_TEST_API_BASE").map_err(|_| {
            LoaError::other(
                "test-hooks ビルドでは LOA_TEST_API_BASE が必要です（実APIには接続しません）",
            )
        })?;
        #[cfg(not(feature = "test-hooks"))]
        let base = API_BASE.to_string();
        Ok(LineApi::with_base(base))
    }

    pub fn with_base(base: impl Into<String>) -> LineApi {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(30)))
            .http_status_as_error(false)
            .user_agent(format!("loa/{}", crate::util::LOA_VERSION))
            .build()
            .into();
        LineApi {
            base: base.into().trim_end_matches('/').to_string(),
            agent,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    fn check(
        &self,
        what: &str,
        resp: &mut ureq::http::Response<ureq::Body>,
    ) -> Result<Option<Value>> {
        let status = resp.status().as_u16();
        let body = resp.body_mut().read_to_string().unwrap_or_default();
        let json: Option<Value> = serde_json::from_str(&body).ok();
        if (200..300).contains(&status) {
            return Ok(json);
        }
        let msg = json
            .as_ref()
            .and_then(|j| {
                j.get("message")
                    .or_else(|| j.get("error_description"))
                    .or_else(|| j.get("error"))
            })
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let msg = crate::redact::redact(&msg);
        let code = if status == 429 {
            Code::RateLimited
        } else {
            Code::ApiError
        };
        let mut e = LoaError::new(code, format!("{what}: HTTP {status} {msg}"))
            .with_detail(serde_json::json!({"status": status}));
        if status == 401 || status == 400 && what.contains("トークン") {
            e = e.with_hint("チャネルIDとチャネルシークレットが正しいか確認してください。loa channel get <channel> で読み直せます");
        }
        Err(e)
    }

    fn transport(e: ureq::Error) -> LoaError {
        LoaError::new(Code::ApiError, format!("LINE APIに接続できません: {e}"))
    }

    /// Stateless channel access token (valid 15 minutes, unlimited issuance).
    pub fn issue_stateless_token(
        &self,
        channel_id: &str,
        secret: &str,
    ) -> Result<Zeroizing<String>> {
        let mut resp = self
            .agent
            .post(&self.url("/oauth2/v3/token"))
            .send_form([
                ("grant_type", "client_credentials"),
                ("client_id", channel_id),
                ("client_secret", secret),
            ])
            .map_err(Self::transport)?;
        let v = self
            .check("トークンの発行", &mut resp)?
            .unwrap_or(Value::Null);
        let token = v
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| LoaError::new(Code::ApiError, "トークンの発行結果を読めません"))?;
        crate::redact::register_secret(token);
        Ok(Zeroizing::new(token.to_string()))
    }

    pub fn liff_list(&self, token: &str) -> Result<Vec<LiffApp>> {
        let mut resp = self
            .agent
            .get(&self.url("/liff/v1/apps"))
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(Self::transport)?;
        // 404 = no LIFF apps on the channel
        if resp.status().as_u16() == 404 {
            return Ok(Vec::new());
        }
        let v = self
            .check("LIFFアプリ一覧の取得", &mut resp)?
            .unwrap_or(Value::Null);
        serde_json::from_value(v.get("apps").cloned().unwrap_or(Value::Array(vec![]))).map_err(
            |e| {
                LoaError::new(
                    Code::ApiError,
                    format!("LIFFアプリ一覧を解釈できません: {e}"),
                )
            },
        )
    }

    pub fn liff_add(&self, token: &str, input: &LiffInput) -> Result<String> {
        input.validate(true)?;
        let mut resp = self
            .agent
            .post(&self.url("/liff/v1/apps"))
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(input.body(true))
            .map_err(Self::transport)?;
        let v = self
            .check("LIFFアプリの追加", &mut resp)?
            .unwrap_or(Value::Null);
        v.get("liffId")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| LoaError::new(Code::ApiError, "追加したLIFF IDを読めません"))
    }

    pub fn liff_update(&self, token: &str, liff_id: &str, input: &LiffInput) -> Result<()> {
        input.validate(false)?;
        let mut resp = self
            .agent
            .put(&self.url(&format!(
                "/liff/v1/apps/{}",
                crate::scenario::template::encode_component(liff_id)
            )))
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(input.body(false))
            .map_err(Self::transport)?;
        self.check("LIFFアプリの更新", &mut resp).map(|_| ())
    }

    pub fn liff_delete(&self, token: &str, liff_id: &str) -> Result<()> {
        let mut resp = self
            .agent
            .delete(&self.url(&format!(
                "/liff/v1/apps/{}",
                crate::scenario::template::encode_component(liff_id)
            )))
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(Self::transport)?;
        self.check("LIFFアプリの削除", &mut resp).map(|_| ())
    }

    pub fn webhook_get(&self, token: &str) -> Result<WebhookInfo> {
        let mut resp = self
            .agent
            .get(&self.url("/v2/bot/channel/webhook/endpoint"))
            .header("Authorization", &format!("Bearer {token}"))
            .call()
            .map_err(Self::transport)?;
        let v = self
            .check("Webhook URLの取得", &mut resp)?
            .unwrap_or(Value::Null);
        Ok(WebhookInfo {
            endpoint: v
                .get("endpoint")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            active: v.get("active").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    pub fn webhook_set(&self, token: &str, endpoint: &str) -> Result<()> {
        if !endpoint.starts_with("https://") {
            return Err(LoaError::usage("Webhook URLは https:// で始めてください"));
        }
        let mut resp = self
            .agent
            .put(&self.url("/v2/bot/channel/webhook/endpoint"))
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(serde_json::json!({ "endpoint": endpoint }))
            .map_err(Self::transport)?;
        self.check("Webhook URLの設定", &mut resp).map(|_| ())
    }

    pub fn webhook_test(&self, token: &str, endpoint: Option<&str>) -> Result<WebhookTestResult> {
        let body = match endpoint {
            Some(e) => serde_json::json!({ "endpoint": e }),
            None => serde_json::json!({}),
        };
        let mut resp = self
            .agent
            .post(&self.url("/v2/bot/channel/webhook/test"))
            .header("Authorization", &format!("Bearer {token}"))
            .send_json(body)
            .map_err(Self::transport)?;
        let v = self
            .check("Webhookのテスト", &mut resp)?
            .unwrap_or(Value::Null);
        serde_json::from_value(v)
            .map_err(|e| LoaError::new(Code::ApiError, format!("テスト結果を解釈できません: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn liff_add_body_has_defaults() {
        let i = LiffInput {
            name: Some("main".into()),
            url: Some("https://stg.example.com".into()),
            size: Some("full".into()),
            ..Default::default()
        };
        let b = i.body(true);
        assert_eq!(b["view"]["type"], "full");
        assert_eq!(b["view"]["url"], "https://stg.example.com");
        assert_eq!(b["description"], "main");
        assert_eq!(b["permanentLinkPattern"], "concat");
        assert_eq!(b["scope"][0], "openid");
    }

    #[test]
    fn liff_update_body_is_partial() {
        let i = LiffInput {
            url: Some("https://x.example".into()),
            ..Default::default()
        };
        let b = i.body(false);
        assert_eq!(b["view"]["url"], "https://x.example");
        assert!(b.get("description").is_none());
        assert!(b.get("scope").is_none());
    }

    #[test]
    fn liff_validation() {
        assert!(
            LiffInput {
                size: Some("huge".into()),
                ..Default::default()
            }
            .validate(false)
            .is_err()
        );
        assert!(
            LiffInput {
                url: Some("http://x".into()),
                ..Default::default()
            }
            .validate(false)
            .is_err()
        );
        assert!(LiffInput::default().validate(true).is_err());
    }
}
