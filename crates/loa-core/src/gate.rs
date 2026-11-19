//! The human gate: every point where a person has to look, approve, paste or log in.

use crate::error::{Code, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalKind {
    /// A destructive or behaviour-changing operation (spec 4-2).
    Operation,
    /// First acceptance (or re-acceptance) of a terms of service (spec 4-3).
    Terms,
    /// One approval covering several operations (selftest on its own provider).
    Bulk,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Change {
    pub field: String,
    #[serde(default)]
    pub before: Option<String>,
    #[serde(default)]
    pub after: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ApprovalRequest {
    pub kind: ApprovalKind,
    /// e.g. "チャネルを削除します"
    pub title: String,
    /// operation id, e.g. "channel.delete"
    pub operation: String,
    /// Target facts shown to the human: (label, value).
    pub target: Vec<(String, String)>,
    #[serde(default)]
    pub changes: Vec<Change>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

impl ApprovalRequest {
    pub fn operation(op: crate::ops::OpKind, title: impl Into<String>) -> Self {
        ApprovalRequest {
            kind: ApprovalKind::Operation,
            title: title.into(),
            operation: op.as_str().to_string(),
            target: Vec::new(),
            changes: Vec::new(),
            link: None,
            note: None,
        }
    }
    pub fn target(mut self, label: &str, value: impl Into<String>) -> Self {
        self.target.push((label.to_string(), value.into()));
        self
    }
    pub fn change(mut self, field: &str, before: Option<String>, after: Option<String>) -> Self {
        self.changes.push(Change {
            field: field.into(),
            before,
            after,
        });
        self
    }
    pub fn note(mut self, n: impl Into<String>) -> Self {
        self.note = Some(n.into());
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct PasteRequest {
    pub title: String,
    /// Where to copy the value from.
    pub instructions: String,
    pub field_label: String,
}

/// Why the task waits for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HumanKind {
    Login,
    Action,
}

impl HumanKind {
    pub fn code(self) -> Code {
        match self {
            HumanKind::Login => Code::LoginRequired,
            HumanKind::Action => Code::HumanActionRequired,
        }
    }
}

/// Condition polled while a human works in the browser. Returns true when done.
pub type DoneCheck<'a> = dyn FnMut() -> Result<bool> + 'a;

pub trait HumanGate {
    /// Show an approval dialog. Ok(true) = approved, Ok(false) = denied or timed out.
    fn approve(&self, req: &ApprovalRequest) -> Result<bool>;

    /// Ask the person to paste a value that cannot be read automatically.
    fn paste(&self, req: &PasteRequest) -> Result<Zeroizing<String>>;

    /// Wait until the person finishes something in the browser (login, SMS, identity check).
    fn wait_human(
        &self,
        kind: HumanKind,
        hint: &str,
        timeout: Duration,
        done: &mut DoneCheck<'_>,
    ) -> Result<()>;

    /// Progress line (stderr in human mode, status file in tasks).
    fn progress(&self, _msg: &str) {}
}

/// Gate that never asks anyone: approvals are denied, human waits fail immediately.
/// Used for `plan` and `--dry-run`, which must not need a person.
pub struct NoHumanGate;

impl HumanGate for NoHumanGate {
    fn approve(&self, _req: &ApprovalRequest) -> Result<bool> {
        Ok(false)
    }
    fn paste(&self, req: &PasteRequest) -> Result<Zeroizing<String>> {
        Err(crate::error::LoaError::new(
            Code::HumanActionRequired,
            format!("{} の入力が必要です", req.field_label),
        ))
    }
    fn wait_human(
        &self,
        kind: HumanKind,
        hint: &str,
        _timeout: Duration,
        done: &mut DoneCheck<'_>,
    ) -> Result<()> {
        if done()? {
            return Ok(());
        }
        Err(crate::error::LoaError::new(kind.code(), hint.to_string()))
    }
}
