//! Static checks run on every scenario before it is executed. They make the Rust side,
//! not the YAML, decide what a scenario may do (spec 7-4, v1.1 change 8).

use super::model::{Check, Locator, OutputType, Scenario, Step};
use super::template::variables;
use crate::error::{LoaError, Result};
use crate::ops::{OpKind, is_destructive_label};
use std::collections::BTreeSet;

pub fn validate(s: &Scenario, expected: OpKind) -> Result<()> {
    let fail = |m: String| {
        Err(LoaError::other(format!(
            "シナリオ {} が不正です: {m}",
            s.id
        )))
    };

    if s.operation != expected.as_str() {
        return fail(format!(
            "operation は {} であるべきところ {} になっています",
            expected.as_str(),
            s.operation
        ));
    }
    let min = semver::Version::parse(&s.min_loa_version)
        .map_err(|_| LoaError::other(format!("シナリオ {} の min_loa_version が不正です", s.id)))?;
    let current = semver::Version::parse(crate::util::LOA_VERSION).expect("crate version");
    if min > current {
        return Err(LoaError::setup(format!(
            "シナリオ {} は loa {} 以上が必要です（現在 {}）",
            s.id, min, current
        ))
        .with_hint("loa 本体を更新してください"));
    }

    let mut known: BTreeSet<String> = s.inputs.keys().cloned().collect();
    known.extend(s.outputs.keys().cloned());
    let mut ctx = Ctx {
        s,
        op: expected,
        known,
        written: BTreeSet::new(),
        confirm_seen: false,
    };
    for step in &s.steps {
        ctx.step(step)?;
    }
    for c in &s.verify {
        ctx.check(c)?;
    }
    for (name, _) in s.outputs.iter() {
        if !ctx.written.contains(name) {
            return fail(format!("出力 {name} に値を書くステップがありません"));
        }
    }
    if expected.needs_target_confirmation() && !ctx.confirm_seen {
        return fail("承認が必要な操作には confirm_target が必要です".into());
    }
    Ok(())
}

struct Ctx<'a> {
    s: &'a Scenario,
    op: OpKind,
    known: BTreeSet<String>,
    written: BTreeSet<String>,
    confirm_seen: bool,
}

impl Ctx<'_> {
    fn err(&self, m: String) -> LoaError {
        LoaError::other(format!("シナリオ {} が不正です: {m}", self.s.id))
    }

    fn vars(&self, text: &str) -> Result<()> {
        for v in variables(text) {
            if !self.known.contains(&v) {
                return Err(self.err(format!("未定義の変数 {{{{{v}}}}}")));
            }
        }
        Ok(())
    }

    fn locator(&self, l: &Locator) -> Result<()> {
        if l.is_empty() {
            return Err(self.err("要素の指定がありません".into()));
        }
        for t in [
            &l.role,
            &l.name,
            &l.label,
            &l.text,
            &l.placeholder,
            &l.testid,
            &l.css,
        ]
        .into_iter()
        .flatten()
        {
            self.vars(t)?;
        }
        if let Some(w) = &l.within {
            if w.css.is_some() {
                return Err(self.err("within には css 以外の指定を使ってください".into()));
            }
            self.locator(w)?;
        }
        Ok(())
    }

    fn output(&mut self, into: &str, secret: bool) -> Result<()> {
        let Some(spec) = self.s.outputs.get(into) else {
            return Err(self.err(format!("into: {into} が outputs に宣言されていません")));
        };
        let is_secret = spec.ty == OutputType::Secret;
        if is_secret != secret {
            return Err(self.err(if is_secret {
                format!("secret 型の {into} には read_secret か paste で書いてください")
            } else {
                format!("{into} は secret 型ではないので read_secret では書けません")
            }));
        }
        self.written.insert(into.to_string());
        Ok(())
    }

    fn read_only_forbids(&self, kind: &str) -> Result<()> {
        if !self.op.is_mutating() {
            return Err(self.err(format!(
                "読み取り操作 {} では {kind} を使えません",
                self.op.as_str()
            )));
        }
        Ok(())
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        match step {
            Step::Goto(u) => {
                self.vars(u)?;
                let after_scheme = u
                    .strip_prefix("https://")
                    .or_else(|| u.strip_prefix("http://"))
                    .ok_or_else(|| self.err(format!("goto は https:// で始めてください: {u}")))?;
                let host = after_scheme.split('/').next().unwrap_or("");
                if host.contains("{{") {
                    return Err(self.err("goto のホスト部分に変数は使えません".into()));
                }
            }
            Step::RequireLogin(_) => {}
            Step::WaitUrl(u) | Step::WaitText(u) => self.vars(u)?,
            Step::WaitFor(l) => self.locator(l)?,
            Step::Click(l) => {
                self.locator(l)?;
                if !self.op.is_mutating() {
                    let role = l.role.as_deref().unwrap_or("").to_ascii_lowercase();
                    if !matches!(role.as_str(), "link" | "tab") {
                        return Err(self.err(format!(
                            "読み取り操作 {} でクリックできるのは role: link / tab だけです",
                            self.op.as_str()
                        )));
                    }
                }
                if let Some(n) = l.visible_name()
                    && is_destructive_label(n)
                    && !self.op.allows_destructive_clicks()
                {
                    return Err(self.err(format!(
                        "{} では「{n}」のような破壊的なボタンを押せません",
                        self.op.as_str()
                    )));
                }
            }
            Step::Fill(f) => {
                self.read_only_forbids("fill")?;
                self.locator(&f.target)?;
                self.vars(&f.value)?;
            }
            Step::Select(v) => {
                self.read_only_forbids("select")?;
                self.locator(&v.target)?;
                self.vars(&v.value)?;
            }
            Step::Check(c) => {
                self.read_only_forbids("check")?;
                self.locator(&c.target)?;
            }
            Step::Upload(u) => {
                self.read_only_forbids("upload")?;
                self.locator(&u.target)?;
                self.vars(&u.file)?;
            }
            Step::Read(r) => {
                self.locator(&r.target)?;
                self.output(&r.into, false)?;
            }
            Step::ReadSecret(r) => {
                self.locator(&r.target)?;
                self.output(&r.into, true)?;
            }
            Step::ReadUrl(r) => {
                regex::Regex::new(&r.pattern)
                    .map_err(|e| self.err(format!("read_url.pattern: {e}")))?;
                self.output(&r.into, false)?;
            }
            Step::ReadList(r) => {
                regex::Regex::new(&r.link_pattern)
                    .map_err(|e| self.err(format!("read_list.link_pattern: {e}")))?;
                self.output(&r.into, false)?;
            }
            Step::Human(h) => {
                self.vars(&h.message)?;
                self.check(&h.until)?;
            }
            Step::AgreeTerms(t) => {
                self.read_only_forbids("agree_terms")?;
                self.locator(&t.target)?;
                if let Some(l) = &t.text_from {
                    self.locator(l)?;
                }
            }
            Step::IfExists(i) => {
                self.locator(&i.target)?;
                for s in i.then.iter().chain(i.otherwise.iter()) {
                    self.step(s)?;
                }
            }
            Step::Assert(c) => self.check(c)?,
            Step::ConfirmTarget(c) => {
                self.locator(&c.target)?;
                self.vars(&c.equals)?;
                self.confirm_seen = true;
            }
            Step::Paste(p) => {
                self.read_only_forbids("paste")?;
                self.output(&p.into, true)?;
            }
            Step::LockedIfExists(l) => self.locator(l)?,
        }
        Ok(())
    }

    fn check(&mut self, c: &Check) -> Result<()> {
        match c {
            Check::UrlMatches(s) | Check::TextContains(s) => self.vars(s),
            Check::Exists(l) | Check::NotExists(l) => self.locator(l),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::model::Scenario;

    fn sc(op: &str, steps: &str, outputs: &str) -> Scenario {
        Scenario::from_yaml(&format!(
            "id: t\nversion: 1\nmin_loa_version: \"0.1.0\"\noperation: {op}\ninputs:\n  name: {{ type: string }}\noutputs:\n{outputs}\nsteps:\n{steps}"
        ))
        .unwrap()
    }

    #[test]
    fn operation_must_match_command() {
        let s = sc(
            "channel.get",
            "  - goto: \"https://developers.line.biz/\"\n",
            "  {}\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_ok());
        assert!(validate(&s, OpKind::ChannelDelete).is_err());
    }

    #[test]
    fn read_only_cannot_fill_or_click_buttons() {
        let s = sc(
            "channel.get",
            "  - fill: { label: \"x\", value: \"y\" }\n",
            "  {}\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_err());
        let s = sc(
            "channel.get",
            "  - click: { role: button, name: \"保存\" }\n",
            "  {}\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_err());
        let s = sc(
            "channel.get",
            "  - click: { role: tab, name: \"LIFF\" }\n",
            "  {}\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_ok());
    }

    #[test]
    fn destructive_click_needs_destructive_op() {
        let s = sc(
            "channel.update",
            "  - click: { role: button, name: \"削除\" }\n",
            "  {}\n",
        );
        assert!(validate(&s, OpKind::ChannelUpdate).is_err());
    }

    #[test]
    fn delete_needs_confirm_target() {
        let steps = "  - click: { role: button, name: \"削除\" }\n";
        let s = sc("channel.delete", steps, "  {}\n");
        assert!(validate(&s, OpKind::ChannelDelete).is_err());
        let steps = "  - confirm_target: { label: \"チャネル名\", equals: \"{{name}}\" }\n  - click: { role: button, name: \"削除\" }\n";
        let s = sc("channel.delete", steps, "  {}\n");
        assert!(validate(&s, OpKind::ChannelDelete).is_ok());
    }

    #[test]
    fn secret_outputs_only_via_read_secret() {
        let s = sc(
            "channel.get",
            "  - read: { label: \"シークレット\", into: secret }\n",
            "  secret: { type: secret }\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_err());
        let s = sc(
            "channel.get",
            "  - read_secret: { label: \"シークレット\", into: secret }\n",
            "  secret: { type: secret }\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_ok());
    }

    #[test]
    fn host_cannot_be_templated() {
        let s = sc(
            "channel.get",
            "  - goto: \"https://{{name}}/x\"\n",
            "  {}\n",
        );
        assert!(validate(&s, OpKind::ChannelGet).is_err());
    }

    #[test]
    fn unknown_variable_rejected() {
        let s = sc("channel.get", "  - wait_text: \"{{nope}}\"\n", "  {}\n");
        assert!(validate(&s, OpKind::ChannelGet).is_err());
    }
}
