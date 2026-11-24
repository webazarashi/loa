//! Operation kinds and the approval policy (spec 4-2). The policy lives here, in Rust;
//! scenario YAML cannot change it.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum OpKind {
    #[serde(rename = "session.login")]
    SessionLogin,
    #[serde(rename = "session.whoami")]
    SessionWhoami,
    #[serde(rename = "oa.list")]
    OaList,
    #[serde(rename = "oa.get")]
    OaGet,
    #[serde(rename = "oa.create")]
    OaCreate,
    #[serde(rename = "oa.update.profile")]
    OaUpdateProfile,
    #[serde(rename = "oa.update.settings")]
    OaUpdateSettings,
    #[serde(rename = "oa.update.members")]
    OaUpdateMembers,
    #[serde(rename = "oa.update.response")]
    OaUpdateResponse,
    #[serde(rename = "oa.enable_messaging_api")]
    OaEnableMessagingApi,
    #[serde(rename = "provider.list")]
    ProviderList,
    #[serde(rename = "provider.get")]
    ProviderGet,
    #[serde(rename = "provider.create")]
    ProviderCreate,
    #[serde(rename = "provider.update")]
    ProviderUpdate,
    #[serde(rename = "channel.list")]
    ChannelList,
    #[serde(rename = "channel.get")]
    ChannelGet,
    #[serde(rename = "channel.create")]
    ChannelCreate,
    #[serde(rename = "channel.update")]
    ChannelUpdate,
    #[serde(rename = "channel.delete")]
    ChannelDelete,
    #[serde(rename = "channel.reissue_secret")]
    ChannelReissueSecret,
    /// Read the long-lived channel access token, issuing it when there is none.
    #[serde(rename = "channel.issue_token")]
    ChannelIssueToken,
    /// Reissue the long-lived channel access token (the old one stops working).
    #[serde(rename = "channel.reissue_token")]
    ChannelReissueToken,
    #[serde(rename = "liff.list")]
    LiffList,
    #[serde(rename = "liff.add")]
    LiffAdd,
    #[serde(rename = "liff.update")]
    LiffUpdate,
    #[serde(rename = "liff.delete")]
    LiffDelete,
    #[serde(rename = "miniapp.get")]
    MiniappGet,
    #[serde(rename = "miniapp.update")]
    MiniappUpdate,
    #[serde(rename = "webhook.get")]
    WebhookGet,
    #[serde(rename = "webhook.set")]
    WebhookSet,
    #[serde(rename = "webhook.test")]
    WebhookTest,
}

/// Facts the policy needs that are only known at run time.
#[derive(Debug, Clone, Default)]
pub struct PolicyContext {
    /// For webhook.set: the endpoint currently configured (None or empty = not set).
    pub existing_value: Option<String>,
    /// For webhook.set: the endpoint that will be configured.
    pub new_value: Option<String>,
}

impl OpKind {
    pub fn as_str(self) -> &'static str {
        match self {
            OpKind::SessionLogin => "session.login",
            OpKind::SessionWhoami => "session.whoami",
            OpKind::OaList => "oa.list",
            OpKind::OaGet => "oa.get",
            OpKind::OaCreate => "oa.create",
            OpKind::OaUpdateProfile => "oa.update.profile",
            OpKind::OaUpdateSettings => "oa.update.settings",
            OpKind::OaUpdateMembers => "oa.update.members",
            OpKind::OaUpdateResponse => "oa.update.response",
            OpKind::OaEnableMessagingApi => "oa.enable_messaging_api",
            OpKind::ProviderList => "provider.list",
            OpKind::ProviderGet => "provider.get",
            OpKind::ProviderCreate => "provider.create",
            OpKind::ProviderUpdate => "provider.update",
            OpKind::ChannelList => "channel.list",
            OpKind::ChannelGet => "channel.get",
            OpKind::ChannelCreate => "channel.create",
            OpKind::ChannelUpdate => "channel.update",
            OpKind::ChannelDelete => "channel.delete",
            OpKind::ChannelReissueSecret => "channel.reissue_secret",
            OpKind::ChannelIssueToken => "channel.issue_token",
            OpKind::ChannelReissueToken => "channel.reissue_token",
            OpKind::LiffList => "liff.list",
            OpKind::LiffAdd => "liff.add",
            OpKind::LiffUpdate => "liff.update",
            OpKind::LiffDelete => "liff.delete",
            OpKind::MiniappGet => "miniapp.get",
            OpKind::MiniappUpdate => "miniapp.update",
            OpKind::WebhookGet => "webhook.get",
            OpKind::WebhookSet => "webhook.set",
            OpKind::WebhookTest => "webhook.test",
        }
    }

    pub fn parse(s: &str) -> Option<OpKind> {
        serde_json::from_value(serde_json::Value::String(s.to_string())).ok()
    }

    /// Does the operation change anything on the LINE side?
    pub fn is_mutating(self) -> bool {
        !matches!(
            self,
            OpKind::SessionWhoami
                | OpKind::OaList
                | OpKind::OaGet
                | OpKind::ProviderList
                | OpKind::ProviderGet
                | OpKind::ChannelList
                | OpKind::ChannelGet
                | OpKind::LiffList
                | OpKind::MiniappGet
                | OpKind::WebhookGet
                | OpKind::WebhookTest
        ) && self != OpKind::SessionLogin
    }

    /// Spec 4-2: does a human have to approve this operation?
    pub fn requires_approval(self, ctx: &PolicyContext) -> bool {
        match self {
            OpKind::ChannelDelete
            | OpKind::LiffDelete
            | OpKind::ChannelReissueSecret
            | OpKind::ChannelReissueToken
            | OpKind::OaUpdateMembers
            | OpKind::OaUpdateResponse
            | OpKind::OaEnableMessagingApi => true,
            OpKind::WebhookSet => {
                let existing = ctx.existing_value.as_deref().unwrap_or("").trim();
                let new = ctx.new_value.as_deref().unwrap_or("").trim();
                !existing.is_empty() && existing != new
            }
            _ => false,
        }
    }

    /// Operations whose scenario may click buttons named like 削除 / 再発行 / Delete.
    pub fn allows_destructive_clicks(self) -> bool {
        matches!(
            self,
            OpKind::ChannelDelete
                | OpKind::ChannelReissueSecret
                | OpKind::ChannelReissueToken
                | OpKind::OaUpdateMembers
        )
    }

    /// Browser operations that need approval must re-check the target on screen before submitting.
    pub fn needs_target_confirmation(self) -> bool {
        self.requires_approval(&PolicyContext {
            existing_value: Some("x".into()),
            new_value: Some("y".into()),
        }) && !matches!(self, OpKind::LiffDelete | OpKind::WebhookSet)
    }
}

/// Button / link names that destroy or rotate something.
pub fn is_destructive_label(name: &str) -> bool {
    const WORDS: [&str; 14] = [
        "削除",
        "再発行",
        "無効",
        "解除",
        "退出",
        "取り消",
        "リセット",
        "delete",
        "remove",
        "reissue",
        "revoke",
        "disable",
        "reset",
        "deactivate",
    ];
    let lower = name.to_lowercase();
    WORDS.iter().any(|w| lower.contains(w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_table_matches_spec() {
        let none = PolicyContext::default();
        for op in [
            OpKind::ChannelDelete,
            OpKind::LiffDelete,
            OpKind::ChannelReissueSecret,
            OpKind::ChannelReissueToken,
            OpKind::OaUpdateMembers,
            OpKind::OaUpdateResponse,
            OpKind::OaEnableMessagingApi,
        ] {
            assert!(op.requires_approval(&none), "{op:?}");
        }
        for op in [
            OpKind::ChannelCreate,
            OpKind::ProviderCreate,
            OpKind::OaUpdateProfile,
            OpKind::OaUpdateSettings,
            OpKind::LiffAdd,
            OpKind::ChannelIssueToken,
        ] {
            assert!(!op.requires_approval(&none), "{op:?}");
        }
    }

    #[test]
    fn webhook_needs_approval_only_when_overwriting() {
        let first = PolicyContext {
            existing_value: None,
            new_value: Some("https://a".into()),
        };
        assert!(!OpKind::WebhookSet.requires_approval(&first));
        let same = PolicyContext {
            existing_value: Some("https://a".into()),
            new_value: Some("https://a".into()),
        };
        assert!(!OpKind::WebhookSet.requires_approval(&same));
        let change = PolicyContext {
            existing_value: Some("https://a".into()),
            new_value: Some("https://b".into()),
        };
        assert!(OpKind::WebhookSet.requires_approval(&change));
    }

    #[test]
    fn op_names_roundtrip() {
        for s in ["channel.create", "oa.update.members", "liff.delete"] {
            assert_eq!(OpKind::parse(s).unwrap().as_str(), s);
        }
        assert!(OpKind::parse("channel.nuke").is_none());
    }

    #[test]
    fn destructive_labels() {
        assert!(is_destructive_label("チャネルを削除"));
        assert!(is_destructive_label("Reissue"));
        assert!(!is_destructive_label("作成"));
    }
}
