//! Scenario YAML format (spec 7-1, v1.1 additions: status, read_url, read_list, confirm_target, paste).

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub version: u32,
    pub min_loa_version: String,
    /// Operation id. Rust checks it equals the operation the command expects.
    pub operation: String,
    /// `verified` = confirmed on the real screens; `provisional` = selectors not yet confirmed.
    #[serde(default)]
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub inputs: BTreeMap<String, InputSpec>,
    #[serde(default)]
    pub outputs: BTreeMap<String, OutputSpec>,
    pub steps: Vec<Step>,
    #[serde(default)]
    pub verify: Vec<Check>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Verified,
    #[default]
    Provisional,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InputSpec {
    #[serde(rename = "type")]
    pub ty: InputType,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InputType {
    String,
    Url,
    File,
    Bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    #[serde(rename = "type")]
    pub ty: OutputType,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutputType {
    String,
    /// Stored in the keychain, never logged or printed.
    Secret,
    List,
}

/// Element locator. Priority: role+name → label → text → placeholder/testid → css (spec 7-2).
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Locator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub testid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub css: Option<String>,
    /// Restrict the match to descendants of this element (e.g. a table row).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub within: Option<Box<Locator>>,
}

impl Locator {
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        if let Some(r) = &self.role {
            parts.push(format!("role={r}"));
        }
        if let Some(n) = &self.name {
            parts.push(format!("name={n}"));
        }
        if let Some(l) = &self.label {
            parts.push(format!("label={l}"));
        }
        if let Some(t) = &self.text {
            parts.push(format!("text={t}"));
        }
        if let Some(p) = &self.placeholder {
            parts.push(format!("placeholder={p}"));
        }
        if let Some(t) = &self.testid {
            parts.push(format!("testid={t}"));
        }
        if let Some(c) = &self.css {
            parts.push(format!("css={c}"));
        }
        if let Some(w) = &self.within {
            parts.push(format!("within({})", w.describe()));
        }
        parts.join(" ")
    }

    /// Visible name used for the destructive-click guard.
    pub fn visible_name(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or(self.text.as_deref())
            .or(self.label.as_deref())
    }

    pub fn is_empty(&self) -> bool {
        self.role.is_none()
            && self.label.is_none()
            && self.text.is_none()
            && self.placeholder.is_none()
            && self.testid.is_none()
            && self.css.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    /// Open a URL (must be on the allowed domains).
    Goto(String),
    /// If the browser is on a login page, hand over to the person and wait.
    RequireLogin(bool),
    /// Wait for the URL to match a glob.
    WaitUrl(String),
    WaitFor(Locator),
    WaitText(String),
    Click(Locator),
    Fill(FillStep),
    Select(ValueStep),
    Check(CheckStep),
    Upload(UploadStep),
    Read(ReadStep),
    ReadSecret(ReadStep),
    /// Capture part of the current URL (e.g. the new channel id).
    ReadUrl(ReadUrlStep),
    /// Collect links whose path matches a pattern (resource lists).
    ReadList(ReadListStep),
    /// Show a message and wait until the person finished (SMS, identity check).
    Human(HumanStep),
    /// Terms of service checkbox: first time approved by a person, then automatic.
    AgreeTerms(TermsStep),
    IfExists(IfExistsStep),
    Assert(Check),
    /// Re-read the target on screen and stop unless it equals the approved value.
    ConfirmTarget(ConfirmStep),
    /// Ask the person to paste a value that cannot be read automatically.
    Paste(PasteStep),
    /// Stop with LOCKED_BY_REVIEW when the element exists (MINI App under review / published).
    LockedIfExists(Locator),
    /// Run `then` when an input variable is non-empty (optional fields), otherwise `else`.
    IfSet(IfSetStep),
}

impl Step {
    pub fn kind(&self) -> &'static str {
        match self {
            Step::Goto(_) => "goto",
            Step::RequireLogin(_) => "require_login",
            Step::WaitUrl(_) => "wait_url",
            Step::WaitFor(_) => "wait_for",
            Step::WaitText(_) => "wait_text",
            Step::Click(_) => "click",
            Step::Fill(_) => "fill",
            Step::Select(_) => "select",
            Step::Check(_) => "check",
            Step::Upload(_) => "upload",
            Step::Read(_) => "read",
            Step::ReadSecret(_) => "read_secret",
            Step::ReadUrl(_) => "read_url",
            Step::ReadList(_) => "read_list",
            Step::Human(_) => "human",
            Step::AgreeTerms(_) => "agree_terms",
            Step::IfExists(_) => "if_exists",
            Step::Assert(_) => "assert",
            Step::ConfirmTarget(_) => "confirm_target",
            Step::Paste(_) => "paste",
            Step::LockedIfExists(_) => "locked_if_exists",
            Step::IfSet(_) => "if_set",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct FillStep {
    #[serde(flatten)]
    pub target: Locator,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ValueStep {
    #[serde(flatten)]
    pub target: Locator,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CheckStep {
    #[serde(flatten)]
    pub target: Locator,
    /// true / false / on / off, or a `{{variable}}` holding one of them
    #[serde(default = "yes", deserialize_with = "bool_or_string")]
    pub checked: String,
}

fn yes() -> String {
    "true".into()
}

fn bool_or_string<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum B {
        B(bool),
        S(String),
    }
    Ok(match B::deserialize(d)? {
        B::B(b) => b.to_string(),
        B::S(s) => s,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UploadStep {
    #[serde(flatten)]
    pub target: Locator,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ReadStep {
    #[serde(flatten)]
    pub target: Locator,
    pub into: String,
    /// text (default) | value | href | attribute name
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attr: Option<String>,
    /// Regex; the first capture group (or the whole match) becomes the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
    /// When true, a missing element yields an empty value instead of UI_CHANGED.
    #[serde(default)]
    pub optional: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReadUrlStep {
    pub pattern: String,
    pub into: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReadListStep {
    /// Regex applied to each link's path; `(?<id>...)` or the first group is the item id.
    pub link_pattern: String,
    pub into: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HumanStep {
    pub message: String,
    pub until: Check,
    /// login | action (default action)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::gate::HumanKind>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TermsStep {
    /// Stable id of the terms type, e.g. `line-developers-agreement`.
    pub terms_id: String,
    /// Display name shown in the approval dialog.
    pub title: String,
    /// Checkbox (or button) that expresses agreement.
    #[serde(flatten)]
    pub target: Locator,
    /// Element containing the terms text; its hash detects changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_from: Option<Locator>,
    /// Link to the terms, shown in the dialog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// Click instead of check (agreement expressed by a button).
    #[serde(default)]
    pub click: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct IfExistsStep {
    #[serde(flatten)]
    pub target: Locator,
    /// Keep looking this long before taking `else` (elements that appear late, like dialogs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait_ms: Option<u64>,
    #[serde(default)]
    pub then: Vec<Step>,
    #[serde(default, rename = "else")]
    pub otherwise: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IfSetStep {
    pub var: String,
    #[serde(default)]
    pub then: Vec<Step>,
    #[serde(default, rename = "else")]
    pub otherwise: Vec<Step>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct ConfirmStep {
    #[serde(flatten)]
    pub target: Locator,
    pub equals: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PasteStep {
    pub title: String,
    pub instructions: String,
    pub field_label: String,
    pub into: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    UrlMatches(String),
    TextContains(String),
    Exists(Locator),
    NotExists(Locator),
}

impl Scenario {
    pub fn from_yaml(s: &str) -> crate::error::Result<Scenario> {
        serde_saphyr::from_str(s)
            .map_err(|e| crate::error::LoaError::other(format!("シナリオYAMLを読めません: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC_EXAMPLE: &str = r#"
id: channel.create.login
version: 3
min_loa_version: "0.1.0"
operation: channel.create
inputs:
  provider_id: { type: string, required: true }
  name:        { type: string, required: true }
outputs:
  channel_id:     { type: string }
  channel_secret: { type: secret }
steps:
  - goto: "https://developers.line.biz/console/provider/{{provider_id}}"
  - require_login: true
  - click: { role: button, name: "新規チャネル作成" }
  - click: { text: "LINEログイン" }
  - fill:  { label: "チャネル名", value: "{{name}}" }
  - agree_terms: { terms_id: "line-developers-agreement", title: "LINE Developers Agreement", label: "利用規約に同意" }
  - click: { role: button, name: "作成" }
  - wait_url: "**/channel/*/basics"
  - read:        { label: "チャネルID", into: channel_id }
  - read_secret: { label: "チャネルシークレット", into: channel_secret }
  - if_exists:
      role: dialog
      name: "確認"
      then:
        - click: { role: button, name: "OK" }
verify:
  - text_contains: "{{name}}"
"#;

    #[test]
    fn parses_spec_example() {
        let s = Scenario::from_yaml(SPEC_EXAMPLE).unwrap();
        assert_eq!(s.operation, "channel.create");
        assert_eq!(s.status, Status::Provisional);
        assert_eq!(s.steps.len(), 11);
        match &s.steps[4] {
            Step::Fill(f) => {
                assert_eq!(f.target.label.as_deref(), Some("チャネル名"));
                assert_eq!(f.value, "{{name}}");
            }
            other => panic!("{other:?}"),
        }
        match &s.steps[5] {
            Step::AgreeTerms(t) => assert_eq!(t.target.label.as_deref(), Some("利用規約に同意")),
            other => panic!("{other:?}"),
        }
        match &s.steps[10] {
            Step::IfExists(i) => assert_eq!(i.then.len(), 1),
            other => panic!("{other:?}"),
        }
        assert_eq!(s.outputs["channel_secret"].ty, OutputType::Secret);
    }

    #[test]
    fn rejects_unknown_step() {
        let y = "id: x\nversion: 1\nmin_loa_version: \"0.1.0\"\noperation: channel.get\nsteps:\n  - eval: \"alert(1)\"\n";
        assert!(Scenario::from_yaml(y).is_err());
    }
}
