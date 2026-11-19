//! Error codes, exit codes and the error type shared by every command.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Machine readable error code. The exit code is fixed per code (spec 3-5, plus v1.1 additions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Code {
    Error,
    Usage,
    LoginRequired,
    HumanActionRequired,
    ApprovalRequired,
    ApprovalDenied,
    WaitTimeout,
    WorkerLost,
    UiChanged,
    LockedByReview,
    DomainBlocked,
    RateLimited,
    Busy,
    ApiError,
    SetupRequired,
}

impl Code {
    pub const ALL: [Code; 15] = [
        Code::Error,
        Code::Usage,
        Code::LoginRequired,
        Code::HumanActionRequired,
        Code::ApprovalRequired,
        Code::ApprovalDenied,
        Code::WaitTimeout,
        Code::WorkerLost,
        Code::UiChanged,
        Code::LockedByReview,
        Code::DomainBlocked,
        Code::RateLimited,
        Code::Busy,
        Code::ApiError,
        Code::SetupRequired,
    ];

    pub fn exit_code(self) -> i32 {
        match self {
            Code::Error => 1,
            Code::Usage => 2,
            Code::LoginRequired => 10,
            Code::HumanActionRequired => 11,
            Code::ApprovalRequired => 12,
            Code::ApprovalDenied => 13,
            Code::WaitTimeout => 14,
            Code::WorkerLost => 15,
            Code::UiChanged => 20,
            Code::LockedByReview => 21,
            Code::DomainBlocked => 22,
            Code::RateLimited => 30,
            Code::Busy => 31,
            Code::ApiError => 40,
            Code::SetupRequired => 50,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Code::Error => "ERROR",
            Code::Usage => "USAGE",
            Code::LoginRequired => "LOGIN_REQUIRED",
            Code::HumanActionRequired => "HUMAN_ACTION_REQUIRED",
            Code::ApprovalRequired => "APPROVAL_REQUIRED",
            Code::ApprovalDenied => "APPROVAL_DENIED",
            Code::WaitTimeout => "WAIT_TIMEOUT",
            Code::WorkerLost => "WORKER_LOST",
            Code::UiChanged => "UI_CHANGED",
            Code::LockedByReview => "LOCKED_BY_REVIEW",
            Code::DomainBlocked => "DOMAIN_BLOCKED",
            Code::RateLimited => "RATE_LIMITED",
            Code::Busy => "BUSY",
            Code::ApiError => "API_ERROR",
            Code::SetupRequired => "SETUP_REQUIRED",
        }
    }

    /// Codes that mean "a human has to do something; the task keeps running".
    pub fn is_waiting(self) -> bool {
        matches!(
            self,
            Code::LoginRequired | Code::HumanActionRequired | Code::ApprovalRequired
        )
    }

    /// Default next step shown when an error carries no specific one.
    pub fn default_hint(self) -> &'static str {
        match self {
            Code::Error => "メッセージを確認してください",
            Code::Usage => "loa schema <command> で引数を確認してください",
            Code::LoginRequired => "ブラウザでLINEビジネスIDにログインしてください",
            Code::HumanActionRequired => "ブラウザまたはダイアログの案内に従って操作してください",
            Code::ApprovalRequired => "承認ダイアログで内容を確認してください",
            Code::ApprovalDenied => "承認が拒否されたため中止しました",
            Code::WaitTimeout => "待機時間を過ぎました。タスクはまだ続いています",
            Code::WorkerLost => {
                "タスクを実行していたプロセスが終了しました。同じコマンドを再実行してください"
            }
            Code::UiChanged => {
                "画面が想定と違います。loa scenarios update を試し、直らなければIssueを作成してください"
            }
            Code::LockedByReview => {
                "審査中または公開済みのため変更できません。状態を確認してください"
            }
            Code::DomainBlocked => "許可されていないドメインへ移動しようとしたため停止しました",
            Code::RateLimited => "回数制限に達しました。時間をおいて再実行してください",
            Code::Busy => {
                "同じLINEビジネスIDで別の操作が実行中です。終わってから再実行してください"
            }
            Code::ApiError => "LINE APIがエラーを返しました。メッセージを確認してください",
            Code::SetupRequired => "loa setup を実行してください",
        }
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The error every loa operation returns. It is rendered as `{ok:false,...}` in `--json` mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, thiserror::Error)]
#[error("{code}: {message}")]
pub struct LoaError {
    pub code: Code,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<String>,
    /// Local path of a failure report (UI_CHANGED and friends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<serde_json::Value>,
}

impl LoaError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        LoaError {
            code,
            message: message.into(),
            hint: None,
            task: None,
            next: Vec::new(),
            report: None,
            detail: None,
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new(Code::Usage, message)
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(Code::Error, message)
    }

    pub fn ui_changed(message: impl Into<String>) -> Self {
        Self::new(Code::UiChanged, message)
    }

    pub fn setup(message: impl Into<String>) -> Self {
        Self::new(Code::SetupRequired, message).with_next("loa setup")
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_next(mut self, next: impl Into<String>) -> Self {
        self.next.push(next.into());
        self
    }

    pub fn with_task(mut self, task: impl Into<String>) -> Self {
        self.task = Some(task.into());
        self
    }

    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        self.detail = Some(detail);
        self
    }

    pub fn exit_code(&self) -> i32 {
        self.code.exit_code()
    }

    pub fn hint_or_default(&self) -> String {
        self.hint
            .clone()
            .unwrap_or_else(|| self.code.default_hint().to_string())
    }
}

impl From<std::io::Error> for LoaError {
    fn from(e: std::io::Error) -> Self {
        LoaError::other(format!("入出力エラー: {e}"))
    }
}

impl From<serde_json::Error> for LoaError {
    fn from(e: serde_json::Error) -> Self {
        LoaError::other(format!("JSONの処理に失敗しました: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, LoaError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn exit_codes_are_unique_and_match_spec() {
        let codes: HashSet<i32> = Code::ALL.iter().map(|c| c.exit_code()).collect();
        assert_eq!(codes.len(), Code::ALL.len());
        assert_eq!(Code::LoginRequired.exit_code(), 10);
        assert_eq!(Code::UiChanged.exit_code(), 20);
        assert_eq!(Code::ApiError.exit_code(), 40);
        assert_eq!(Code::SetupRequired.exit_code(), 50);
    }

    #[test]
    fn codes_serialize_as_screaming_snake() {
        for c in Code::ALL {
            let s = serde_json::to_string(&c).unwrap();
            assert_eq!(s, format!("\"{}\"", c.as_str()));
        }
    }
}
