//! Executes a validated scenario step by step through the [`Browser`] driver.

use super::model::{Check, Locator, ReadStep, Scenario, Step, TermsStep};
use super::template::{Vars, render, render_url};
use crate::browser::{Browser, BrowserErrorKind, Cmd, Snapshot, to_loa_error};
use crate::domains::{Allowlist, glob_match, is_login_url};
use crate::error::{Code, LoaError, Result};
use crate::gate::{ApprovalKind, ApprovalRequest, HumanGate, HumanKind, PasteRequest};
use crate::ops::{OpKind, is_destructive_label};
use crate::terms::{TermsStore, hash_text};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Default)]
pub struct Outputs {
    pub values: BTreeMap<String, Value>,
    pub secrets: BTreeMap<String, Zeroizing<String>>,
}

impl Outputs {
    pub fn str(&self, k: &str) -> Option<String> {
        self.values
            .get(k)
            .and_then(|v| v.as_str())
            .map(str::to_string)
    }
}

pub struct Runner<'a> {
    pub browser: &'a mut dyn Browser,
    pub gate: &'a dyn HumanGate,
    pub terms: &'a TermsStore,
    pub allow: Allowlist,
    pub op: OpKind,
    pub human_timeout: Duration,
    pub wait_timeout: Duration,
    pub reports_dir: PathBuf,
    vars: Vars,
    out: Outputs,
    last_goto: Option<String>,
}

enum Target {
    /// `find <kind> <value> ... [--name N] --exact`
    Find {
        kind: String,
        value: String,
        name: Option<String>,
    },
    /// CSS selector or `@eN` ref.
    Sel(String),
}

impl<'a> Runner<'a> {
    pub fn new(
        browser: &'a mut dyn Browser,
        gate: &'a dyn HumanGate,
        terms: &'a TermsStore,
        op: OpKind,
        reports_dir: PathBuf,
    ) -> Self {
        Runner {
            browser,
            gate,
            terms,
            allow: Allowlist::default_for_build(),
            op,
            human_timeout: Duration::from_secs(1800),
            wait_timeout: Duration::from_secs(15),
            reports_dir,
            vars: Vars::new(),
            out: Outputs::default(),
            last_goto: None,
        }
    }

    pub fn run(mut self, sc: &Scenario, inputs: &Vars) -> Result<Outputs> {
        super::validate::validate(sc, self.op)?;
        // inputs: declared defaults, then provided values
        for (k, spec) in &sc.inputs {
            if let Some(v) = inputs.get(k) {
                self.vars.insert(k.clone(), v.clone());
            } else if let Some(d) = &spec.default {
                self.vars.insert(k.clone(), d.clone());
            } else if spec.required {
                return Err(LoaError::usage(format!(
                    "シナリオ {} の入力 {k} がありません",
                    sc.id
                )));
            }
        }
        for (i, step) in sc.steps.iter().enumerate() {
            self.gate
                .progress(&format!("[{}/{}] {}", i + 1, sc.steps.len(), step.kind()));
            if let Err(e) = self.step(step) {
                return Err(self.fail(sc, &format!("{}", i + 1), step.kind(), e));
            }
        }
        for (i, c) in sc.verify.iter().enumerate() {
            let ok = self.poll_check(c)?;
            if !ok {
                let e = LoaError::ui_changed("操作後の確認（verify）に失敗しました");
                return Err(self.fail(sc, &format!("verify[{}]", i + 1), "verify", e));
            }
        }
        Ok(self.out)
    }

    fn fail(&mut self, sc: &Scenario, step_no: &str, kind: &str, e: LoaError) -> LoaError {
        if !matches!(e.code, Code::UiChanged | Code::DomainBlocked | Code::Error) {
            return e;
        }
        let url = self.browser.url().unwrap_or_default();
        let snap = self.browser.snapshot().ok();
        let path = crate::report::write_failure(
            &self.reports_dir,
            sc,
            step_no,
            kind,
            &url,
            &e,
            snap.as_ref(),
            self.browser,
        );
        let mut e = e;
        e.message = format!(
            "{}（シナリオ {} のステップ {step_no}: {kind}）",
            e.message, sc.id
        );
        if let Some(p) = path {
            e.report = Some(p.display().to_string());
        }
        if e.code == Code::UiChanged {
            if e.next.is_empty() {
                e.next.push("loa scenarios update".into());
            }
            if sc.status == super::model::Status::Provisional {
                e.hint = Some(
                    "このシナリオは実画面で未確認（provisional）です。記録を添えてIssueを作成してください"
                        .into(),
                );
            }
        }
        e
    }

    // ---------------------------------------------------------------- steps

    fn step(&mut self, step: &Step) -> Result<()> {
        // Client-side redirects can finish after the previous step's check: verify the page
        // before every step that reads or acts on it.
        if !matches!(step, Step::Goto(_)) && self.last_goto.is_some() {
            self.check_location()?;
        }
        match step {
            Step::Goto(u) => {
                let url = crate::domains::test_rewrite(&render_url(u, &self.vars)?);
                self.allow.check(&url)?;
                self.exec(vec!["open".into(), url.clone()])?;
                self.last_goto = Some(url);
                self.check_location()?;
            }
            Step::RequireLogin(on) => {
                if *on {
                    self.require_login()?;
                }
            }
            Step::WaitUrl(g) => {
                let g = render(g, &self.vars)?;
                let deadline = Instant::now() + self.wait_timeout.max(Duration::from_secs(30));
                loop {
                    let u = self.browser.url().map_err(to_loa_error)?;
                    if glob_match(&g, &u) {
                        break;
                    }
                    self.allow.check(&u)?;
                    if Instant::now() > deadline {
                        return Err(LoaError::ui_changed(format!(
                            "URLが {g} になりませんでした"
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(400));
                }
                self.check_location()?;
            }
            Step::WaitFor(l) => {
                let l = self.render_locator(l)?;
                if !self.poll(|r| r.exists(&l))? {
                    return Err(LoaError::ui_changed(format!(
                        "要素が現れません: {}",
                        l.describe()
                    )));
                }
            }
            Step::WaitText(t) => {
                let t = render(t, &self.vars)?;
                let c = Check::TextContains(t.clone());
                if !self.poll_check(&c)? {
                    return Err(LoaError::ui_changed(format!("「{t}」が表示されません")));
                }
            }
            Step::Click(l) => {
                let l = self.render_locator(l)?;
                self.guard_destructive(&l)?;
                self.act(&l, "click", None)?;
                self.check_location()?;
            }
            Step::Fill(f) => {
                let l = self.render_locator(&f.target)?;
                let v = render(&f.value, &self.vars)?;
                self.act(&l, "fill", Some(v))?;
                self.check_location()?;
            }
            Step::Select(s) => {
                let l = self.render_locator(&s.target)?;
                let v = render(&s.value, &self.vars)?;
                let sel = self.selector(&l)?;
                self.exec(vec!["select".into(), sel, v])?;
            }
            Step::Check(c) => {
                let l = self.render_locator(&c.target)?;
                let want = render(&c.checked, &self.vars)?;
                let on = match want.as_str() {
                    "true" | "on" | "yes" => true,
                    "false" | "off" | "no" => false,
                    other => return Err(LoaError::other(format!("check の値が不正です: {other}"))),
                };
                if on {
                    self.act(&l, "check", None)?;
                } else {
                    let sel = self.selector(&l)?;
                    self.exec(vec!["uncheck".into(), sel])?;
                }
            }
            Step::Upload(u) => {
                let l = self.render_locator(&u.target)?;
                let file = render(&u.file, &self.vars)?;
                if !std::path::Path::new(&file).is_file() {
                    return Err(LoaError::usage(format!(
                        "アップロードするファイルがありません: {file}"
                    )));
                }
                let sel = self.selector(&l)?;
                self.exec(vec!["upload".into(), sel, file])?;
            }
            Step::Read(r) => {
                let v = self.read(r)?;
                self.vars.insert(r.into.clone(), v.clone());
                self.out.values.insert(r.into.clone(), Value::String(v));
            }
            Step::ReadSecret(r) => {
                let v = Zeroizing::new(self.read(r)?);
                if v.trim().is_empty() {
                    return Err(LoaError::ui_changed(format!(
                        "{} を読み取れませんでした",
                        r.into
                    )));
                }
                crate::redact::register_secret(&v);
                self.out.secrets.insert(r.into.clone(), v);
            }
            Step::ReadUrl(r) => {
                let u = self.browser.url().map_err(to_loa_error)?;
                let v = extract(&r.pattern, &u).ok_or_else(|| {
                    LoaError::ui_changed(format!("URLから {} を読み取れません", r.into))
                })?;
                self.vars.insert(r.into.clone(), v.clone());
                self.out.values.insert(r.into.clone(), Value::String(v));
            }
            Step::ReadList(r) => {
                let js = list_js(&r.link_pattern);
                let v = self.eval(&js)?;
                self.out.values.insert(r.into.clone(), v);
            }
            Step::Human(h) => {
                let msg = render(&h.message, &self.vars)?;
                let until = self.render_check(&h.until)?;
                let kind = h.kind.unwrap_or(HumanKind::Action);
                let gate = self.gate;
                let timeout = self.human_timeout;
                gate.wait_human(kind, &msg, timeout, &mut || self.eval_check(&until))?;
                self.check_location()?;
            }
            Step::AgreeTerms(t) => self.agree_terms(t)?,
            Step::IfExists(i) => {
                let l = self.render_locator(&i.target)?;
                let deadline =
                    Instant::now() + Duration::from_millis(i.wait_ms.unwrap_or(0).min(30_000));
                let found = loop {
                    if self.exists(&l)? {
                        break true;
                    }
                    if Instant::now() >= deadline {
                        break false;
                    }
                    std::thread::sleep(Duration::from_millis(300));
                };
                let branch = if found { &i.then } else { &i.otherwise };
                for s in branch {
                    self.step(s)?;
                }
            }
            Step::Assert(c) => {
                let c = self.render_check(c)?;
                if !self.poll_check(&c)? {
                    return Err(LoaError::ui_changed(format!(
                        "想定した状態になっていません: {c:?}"
                    )));
                }
            }
            Step::ConfirmTarget(c) => {
                let expected = render(&c.equals, &self.vars)?;
                let r = ReadStep {
                    target: c.target.clone(),
                    into: "_confirm".into(),
                    attr: None,
                    pattern: c.pattern.clone(),
                    optional: false,
                };
                let actual = self.read(&r)?;
                if crate::util::norm(&actual) != crate::util::norm(&expected) {
                    return Err(LoaError::ui_changed(format!(
                        "画面上の対象（{}）が承認した対象（{}）と一致しないため中止しました",
                        crate::redact::redact(&actual),
                        expected
                    )));
                }
            }
            Step::Paste(p) => {
                let v = self.gate.paste(&PasteRequest {
                    title: render(&p.title, &self.vars)?,
                    instructions: render(&p.instructions, &self.vars)?,
                    field_label: render(&p.field_label, &self.vars)?,
                })?;
                crate::redact::register_secret(&v);
                self.out.secrets.insert(p.into.clone(), v);
            }
            Step::IfSet(i) => {
                let set = self.vars.get(&i.var).is_some_and(|v| !v.trim().is_empty());
                let branch = if set { &i.then } else { &i.otherwise };
                for s in branch {
                    self.step(s)?;
                }
            }
            Step::LockedIfExists(l) => {
                let l = self.render_locator(l)?;
                if self.exists(&l)? {
                    return Err(LoaError::new(
                        Code::LockedByReview,
                        "審査中または公開済みのため変更できません",
                    ));
                }
            }
        }
        Ok(())
    }

    fn require_login(&mut self) -> Result<()> {
        let url = self.browser.url().map_err(to_loa_error)?;
        if !is_login_url(&url) {
            return Ok(());
        }
        let gate = self.gate;
        let timeout = self.human_timeout;
        let allow = self.allow.clone();
        let browser = &mut *self.browser;
        gate.wait_human(
            HumanKind::Login,
            "ブラウザでLINEビジネスIDにログインしてください",
            timeout,
            &mut || {
                let u = browser.url().map_err(to_loa_error)?;
                Ok(!u.is_empty()
                    && !is_login_url(&u)
                    && allow.check(&u).is_ok()
                    && u != "about:blank")
            },
        )?;
        if let Some(g) = self.last_goto.clone() {
            self.exec(vec!["open".into(), g])?;
        }
        self.check_location()
    }

    fn agree_terms(&mut self, t: &TermsStep) -> Result<()> {
        let target = self.render_locator(&t.target)?;
        let link = match &t.link {
            Some(l) => Some(render(l, &self.vars)?),
            None => None,
        };
        let text = match &t.text_from {
            Some(l) => {
                let l = self.render_locator(l)?;
                self.read(&ReadStep {
                    target: l,
                    into: "_terms".into(),
                    attr: None,
                    pattern: None,
                    optional: false,
                })?
            }
            None => format!("{}\n{}", t.title, link.clone().unwrap_or_default()),
        };
        let hash = hash_text(&text);
        if !self.terms.is_approved(&t.terms_id, &hash)? {
            let previously = self
                .terms
                .records()?
                .into_iter()
                .any(|r| r.terms_id == t.terms_id);
            let req = ApprovalRequest {
                kind: ApprovalKind::Terms,
                title: if previously {
                    format!("「{}」の内容が変わりました。同意しますか？", t.title)
                } else {
                    format!("「{}」に同意しますか？", t.title)
                },
                operation: self.op.as_str().to_string(),
                target: vec![
                    ("規約".into(), t.title.clone()),
                    ("内容のハッシュ".into(), hash[..12].to_string()),
                ],
                changes: Vec::new(),
                link: link.clone(),
                note: Some("同意すると、同じ内容の規約には以後 loa が自動で同意します。".into()),
            };
            if !self.gate.approve(&req)? {
                return Err(LoaError::new(
                    Code::ApprovalDenied,
                    format!("「{}」への同意が拒否されました", t.title),
                ));
            }
            self.terms.record(&t.terms_id, &t.title, &hash, link)?;
        }
        if t.click {
            self.act(&target, "click", None)
        } else {
            self.act(&target, "check", None)
        }
    }

    // ---------------------------------------------------------------- helpers

    fn guard_destructive(&self, l: &Locator) -> Result<()> {
        if let Some(n) = l.visible_name()
            && is_destructive_label(n)
            && !self.op.allows_destructive_clicks()
        {
            return Err(LoaError::other(format!(
                "{} では「{n}」を押せません（破壊的な操作は承認付きの専用コマンドだけが行えます）",
                self.op.as_str()
            )));
        }
        Ok(())
    }

    fn check_location(&mut self) -> Result<()> {
        let u = self.browser.url().map_err(to_loa_error)?;
        // An unreadable location must not pass as "allowed".
        if u.trim().is_empty() {
            return Err(LoaError::new(
                Code::DomainBlocked,
                "現在のURLを確認できないため停止しました",
            ));
        }
        self.allow.check(&u)
    }

    fn exec(&mut self, c: Cmd) -> Result<Value> {
        self.browser.run(c).map_err(to_loa_error)
    }

    fn eval(&mut self, js: &str) -> Result<Value> {
        let v = self.exec(vec!["eval".into(), js.to_string()])?;
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    fn render_locator(&self, l: &Locator) -> Result<Locator> {
        let r = |o: &Option<String>| -> Result<Option<String>> {
            o.as_ref().map(|s| render(s, &self.vars)).transpose()
        };
        Ok(Locator {
            role: r(&l.role)?,
            name: r(&l.name)?,
            label: r(&l.label)?,
            text: r(&l.text)?,
            placeholder: r(&l.placeholder)?,
            testid: r(&l.testid)?,
            css: r(&l.css)?,
            within: match &l.within {
                Some(w) => Some(Box::new(self.render_locator(w)?)),
                None => None,
            },
        })
    }

    fn render_check(&self, c: &Check) -> Result<Check> {
        Ok(match c {
            Check::UrlMatches(s) => Check::UrlMatches(render(s, &self.vars)?),
            Check::TextContains(s) => Check::TextContains(render(s, &self.vars)?),
            Check::Exists(l) => Check::Exists(self.render_locator(l)?),
            Check::NotExists(l) => Check::NotExists(self.render_locator(l)?),
        })
    }

    fn target(&self, l: &Locator) -> Target {
        if l.within.is_none() {
            if let Some(role) = &l.role {
                return Target::Find {
                    kind: "role".into(),
                    value: role.clone(),
                    name: l.name.clone(),
                };
            }
            if let Some(v) = &l.label {
                return Target::Find {
                    kind: "label".into(),
                    value: v.clone(),
                    name: None,
                };
            }
            if let Some(v) = &l.text {
                return Target::Find {
                    kind: "text".into(),
                    value: v.clone(),
                    name: None,
                };
            }
            if let Some(v) = &l.placeholder {
                return Target::Find {
                    kind: "placeholder".into(),
                    value: v.clone(),
                    name: None,
                };
            }
            if let Some(v) = &l.testid {
                return Target::Find {
                    kind: "testid".into(),
                    value: v.clone(),
                    name: None,
                };
            }
        }
        Target::Sel(l.css.clone().unwrap_or_default())
    }

    /// click / fill / check with implicit waiting for the element.
    fn act(&mut self, l: &Locator, action: &str, arg: Option<String>) -> Result<()> {
        let c: Cmd = match self.target(l) {
            Target::Find { kind, value, name } => {
                let mut c = vec!["find".into(), kind, value, action.into()];
                if let Some(a) = arg {
                    c.push(a);
                }
                if let Some(n) = name {
                    c.push("--name".into());
                    c.push(n);
                }
                c.push("--exact".into());
                c
            }
            Target::Sel(s) => {
                let sel = if l.within.is_some() || s.is_empty() {
                    self.resolve_ref(l)?
                } else {
                    s
                };
                let mut c = vec![action.to_string(), sel];
                if let Some(a) = arg {
                    c.push(a);
                }
                c
            }
        };
        self.retry_not_found(c).map(|_| ())
    }

    fn retry_not_found(&mut self, c: Cmd) -> Result<Value> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            match self.browser.run(c.clone()) {
                Ok(v) => return Ok(v),
                Err(e) if e.kind == BrowserErrorKind::NotFound && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(500));
                }
                Err(e) => return Err(to_loa_error(e)),
            }
        }
    }

    /// Selector usable by commands that `find` does not support (select, upload, get, uncheck).
    fn selector(&mut self, l: &Locator) -> Result<String> {
        if l.within.is_none() {
            if let Some(c) = &l.css {
                return Ok(c.clone());
            }
            if let Some(p) = &l.placeholder {
                return Ok(format!("[placeholder={}]", css_str(p)));
            }
            if let Some(t) = &l.testid {
                return Ok(format!("[data-testid={}]", css_str(t)));
            }
        }
        self.resolve_ref(l)
    }

    /// Resolve a role / label / text locator to a unique `@eN` ref through the snapshot.
    fn resolve_ref(&mut self, l: &Locator) -> Result<String> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            let snap = self.browser.snapshot().map_err(to_loa_error)?;
            let hits = resolve_in(&snap, l)?;
            match hits.len() {
                1 => return Ok(format!("@{}", hits[0])),
                0 if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(500)),
                0 => {
                    return Err(LoaError::ui_changed(format!(
                        "要素が見つかりません: {}",
                        l.describe()
                    )));
                }
                n => {
                    return Err(LoaError::ui_changed(format!(
                        "要素が {n} 個見つかり特定できません: {}（within で範囲を絞ってください）",
                        l.describe()
                    )));
                }
            }
        }
    }

    fn exists(&mut self, l: &Locator) -> Result<bool> {
        if let (Some(text), Some(w)) = (&l.text, &l.within)
            && l.role.is_none()
        {
            let scope = w.name.clone().or(w.label.clone());
            let js = text_in_scope_js(text, scope.as_deref());
            return Ok(self.eval(&js)?.as_bool().unwrap_or(false));
        }
        match self.target(l) {
            Target::Find { kind, value, name } => {
                let mut c: Cmd = vec!["find".into(), kind, value, "text".into()];
                if let Some(n) = name {
                    c.push("--name".into());
                    c.push(n);
                }
                c.push("--exact".into());
                match self.browser.run(c) {
                    Ok(_) => Ok(true),
                    Err(e) if e.kind == BrowserErrorKind::NotFound => Ok(false),
                    Err(e) if e.message.contains("elements") => Ok(true),
                    Err(e) => Err(to_loa_error(e)),
                }
            }
            Target::Sel(s) if l.within.is_none() && !s.is_empty() => {
                let v = self.exec(vec!["get".into(), "count".into(), s])?;
                Ok(v.get("count").and_then(Value::as_u64).unwrap_or(0) > 0)
            }
            Target::Sel(_) => {
                let snap = self.browser.snapshot().map_err(to_loa_error)?;
                Ok(!resolve_in(&snap, l)?.is_empty())
            }
        }
    }

    fn read(&mut self, r: &ReadStep) -> Result<String> {
        let l = self.render_locator(&r.target)?;
        let attr = r.attr.as_deref().unwrap_or("text");
        let raw = if l.label.is_some() && l.role.is_none() && attr == "text" {
            let scope = l
                .within
                .as_ref()
                .and_then(|w| w.name.clone().or(w.label.clone()));
            self.read_by_label(
                l.label.as_deref().unwrap_or(""),
                scope.as_deref(),
                r.optional,
            )?
        } else {
            let sel = match self.selector(&l) {
                Ok(s) => s,
                Err(e) if r.optional && e.code == Code::UiChanged => return Ok(String::new()),
                Err(e) => return Err(e),
            };
            let c: Cmd = match attr {
                "text" => vec!["get".into(), "text".into(), sel],
                "value" => vec!["get".into(), "value".into(), sel],
                other => vec!["get".into(), "attr".into(), sel, other.into()],
            };
            let v = match self.retry_not_found(c) {
                Ok(v) => v,
                Err(e) if r.optional && e.code == Code::UiChanged => return Ok(String::new()),
                Err(e) => return Err(e),
            };
            ["text", "value", "attr", "result"]
                .iter()
                .find_map(|k| v.get(*k).and_then(Value::as_str))
                .unwrap_or("")
                .to_string()
        };
        let raw = crate::util::norm(&raw);
        match &r.pattern {
            Some(p) => match extract(p, &raw) {
                Some(v) => Ok(v),
                None if r.optional => Ok(String::new()),
                None => Err(LoaError::ui_changed(format!(
                    "{} の値が想定した形式ではありません",
                    r.into
                ))),
            },
            None => Ok(raw),
        }
    }

    fn read_by_label(
        &mut self,
        label: &str,
        scope: Option<&str>,
        optional: bool,
    ) -> Result<String> {
        let js = label_js(label, scope);
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            let v = self.eval(&js)?;
            let count = v.get("count").and_then(Value::as_u64).unwrap_or(0);
            match count {
                1 => {
                    return Ok(v
                        .get("value")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string());
                }
                0 if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(500)),
                0 if optional => return Ok(String::new()),
                0 => {
                    return Err(LoaError::ui_changed(format!(
                        "「{label}」の値が見つかりません"
                    )));
                }
                n => {
                    return Err(LoaError::ui_changed(format!(
                        "「{label}」の値が {n} 通り見つかり特定できません"
                    )));
                }
            }
        }
    }

    fn eval_check(&mut self, c: &Check) -> Result<bool> {
        match c {
            Check::UrlMatches(g) => {
                let u = self.browser.url().map_err(to_loa_error)?;
                Ok(glob_match(g, &u))
            }
            Check::TextContains(t) => {
                let js = format!(
                    "(() => !!document.body && document.body.innerText.includes({}))()",
                    serde_json::to_string(t).unwrap_or_default()
                );
                Ok(self.eval(&js)?.as_bool().unwrap_or(false))
            }
            Check::Exists(l) => self.exists(l),
            Check::NotExists(l) => Ok(!self.exists(l)?),
        }
    }

    fn poll_check(&mut self, c: &Check) -> Result<bool> {
        let c = self.render_check(c)?;
        self.poll(|r| r.eval_check(&c))
    }

    fn poll(&mut self, mut f: impl FnMut(&mut Self) -> Result<bool>) -> Result<bool> {
        let deadline = Instant::now() + self.wait_timeout;
        loop {
            if f(self)? {
                return Ok(true);
            }
            if Instant::now() > deadline {
                return Ok(false);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

/// Refs of nodes matching the locator in a snapshot (exact names, optional `within`).
pub fn resolve_in(snap: &Snapshot, l: &Locator) -> Result<Vec<String>> {
    let scope = match &l.within {
        Some(w) => {
            let parents = matching_nodes(snap, w, None);
            match parents.len() {
                1 => Some(parents[0]),
                0 => return Ok(Vec::new()),
                n => {
                    return Err(LoaError::ui_changed(format!(
                        "範囲指定 {} に {n} 個の要素が当てはまります",
                        w.describe()
                    )));
                }
            }
        }
        None => None,
    };
    Ok(matching_nodes(snap, l, scope)
        .into_iter()
        .filter_map(|n| n.reference.clone())
        .collect())
}

fn matching_nodes<'a>(
    snap: &'a Snapshot,
    l: &Locator,
    scope: Option<&'a crate::browser::Node>,
) -> Vec<&'a crate::browser::Node> {
    use crate::browser::Node;
    fn walk<'b>(n: &'b Node, out: &mut Vec<&'b Node>) {
        out.push(n);
        for c in &n.children {
            walk(c, out);
        }
    }
    let pool: Vec<&Node> = match scope {
        Some(s) => {
            let mut v = Vec::new();
            for c in &s.children {
                walk(c, &mut v);
            }
            v
        }
        None => snap.walk(),
    };
    let name_is = |n: &Node, want: &str| n.name.as_deref() == Some(want);
    if let Some(role) = &l.role {
        return pool
            .into_iter()
            .filter(|n| {
                n.role.eq_ignore_ascii_case(role)
                    && l.name.as_deref().is_none_or(|want| name_is(n, want))
            })
            .collect();
    }
    if let Some(label) = &l.label {
        return pool
            .into_iter()
            .filter(|n| {
                name_is(n, label)
                    && crate::browser::snapshot::LABELLED_ROLES
                        .iter()
                        .any(|r| n.role.eq_ignore_ascii_case(r))
            })
            .collect();
    }
    if let Some(text) = &l.text {
        return pool
            .into_iter()
            .filter(|n| name_is(n, text) && n.reference.is_some())
            .collect();
    }
    Vec::new()
}

fn extract(pattern: &str, text: &str) -> Option<String> {
    let re = regex::Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    if let Some(m) = caps.name("id") {
        return Some(m.as_str().to_string());
    }
    caps.get(1)
        .or_else(|| caps.get(0))
        .map(|m| m.as_str().to_string())
}

fn css_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Fixed script: element named `scope` (aria-label, aria-labelledby or a heading's container).
fn scope_js(scope: Option<&str>) -> String {
    let n = match scope {
        Some(s) => serde_json::to_string(s).unwrap_or_else(|_| "null".into()),
        None => "null".into(),
    };
    format!(
        r#"const findScope = (N) => {{
    if (N === null) return document.body;
    const one = a => a.length === 1 ? a[0] : null;
    const byLabel = one([...document.querySelectorAll('[aria-label]')].filter(e => norm(e.getAttribute('aria-label')) === N));
    if (byLabel) return byLabel;
    const byRef = one([...document.querySelectorAll('[aria-labelledby]')].filter(e => norm(e.getAttribute('aria-labelledby').split(/\s+/).map(i => (document.getElementById(i) || {{}}).innerText || '').join(' ')) === N));
    if (byRef) return byRef;
    const head = one([...document.querySelectorAll('h1,h2,h3,h4,h5,h6,legend,[role=heading]')].filter(h => norm(h.innerText) === N));
    if (head) return head.closest('section,fieldset,article,[role=region],[role=group]') || head.parentElement;
    return null;
  }};
  const SCOPE = findScope({n});"#
    )
}

/// Fixed script: is `text` visible inside the scope?
fn text_in_scope_js(text: &str, scope: Option<&str>) -> String {
    let t = serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(() => {{
  const norm = s => (s || '').replace(/\s+/g, ' ').trim();
  {}
  return !!SCOPE && norm(SCOPE.innerText).includes(norm({t}));
}})()"#,
        scope_js(scope)
    )
}

/// Fixed script: value displayed next to a label (label[for], dl/dt/dd, tables, label/value divs).
fn label_js(label: &str, scope: Option<&str>) -> String {
    let l = serde_json::to_string(label).unwrap_or_else(|_| "\"\"".into());
    let sc = scope_js(scope);
    format!(
        r#"(() => {{
  const L = {l};
  const norm = s => (s || '').replace(/\s+/g, ' ').trim();
  {sc}
  if (!SCOPE) return {{ count: 0, value: null }};
  const own = e => norm([...e.childNodes].filter(n => n.nodeType === 3).map(n => n.textContent).join(' '));
  const valueOf = el => {{
    if (!el) return null;
    const tag = el.tagName;
    if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return el.value;
    const t = norm(el.innerText);
    if (t !== '') return t;
    const inp = el.querySelector && el.querySelector('input,textarea,select');
    return inp ? inp.value : null;
  }};
  const visible = e => !!(e.offsetWidth || e.offsetHeight || e.getClientRects().length) && getComputedStyle(e).visibility !== 'hidden';
  const cands = [...SCOPE.querySelectorAll('*')].filter(e => visible(e) && (own(e) === L || norm(e.innerText) === L) && ![...e.children].some(c => norm(c.innerText) === L));
  const results = [];
  for (const c of cands) {{
    if (c.tagName === 'LABEL' && c.htmlFor) {{
      const t = document.getElementById(c.htmlFor);
      if (t) {{ results.push(valueOf(t)); continue; }}
    }}
    let cur = c, found = null;
    for (let depth = 0; depth < 4 && cur && found === null; depth++) {{
      let sib = cur.nextElementSibling;
      while (sib && found === null) {{
        const v = valueOf(sib);
        if (v !== null && v !== '') found = v;
        sib = sib.nextElementSibling;
      }}
      cur = cur.parentElement;
    }}
    if (found !== null) results.push(found);
  }}
  const uniq = [...new Set(results)];
  return {{ count: uniq.length, value: uniq.length ? uniq[0] : null }};
}})()"#
    )
}

/// Fixed script: links whose path matches the pattern → [{id, name, href}].
fn list_js(pattern: &str) -> String {
    let p = serde_json::to_string(pattern).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"(() => {{
  const re = new RegExp({p});
  const out = []; const seen = new Set();
  for (const a of document.querySelectorAll('a[href]')) {{
    const u = new URL(a.getAttribute('href'), location.href);
    const m = (u.pathname + u.search).match(re);
    if (!m) continue;
    const id = (m.groups && m.groups.id) || m[1] || m[0];
    if (seen.has(id)) continue;
    seen.add(id);
    out.push({{ id, name: (a.innerText || '').replace(/\s+/g, ' ').trim(), href: u.pathname }});
  }}
  return out;
}})()"#
    )
}

pub fn outputs_to_json(o: &Outputs) -> Value {
    let mut m = serde_json::Map::new();
    for (k, v) in &o.values {
        m.insert(k.clone(), v.clone());
    }
    for k in o.secrets.keys() {
        m.insert(k.clone(), json!(crate::redact::MASK));
    }
    Value::Object(m)
}

#[cfg(test)]
// The runner tests use real LINE URLs, which test-hooks builds refuse by design.
#[cfg_attr(feature = "test-hooks", allow(unused_imports, dead_code))]
mod tests {
    use super::*;
    use crate::browser::FakeBrowser;
    use crate::gate::NoHumanGate;
    use crate::scenario::model::Scenario;

    fn scenario(op: &str, body: &str) -> Scenario {
        Scenario::from_yaml(&format!(
            "id: t\nversion: 1\nmin_loa_version: \"0.1.0\"\noperation: {op}\n{body}"
        ))
        .unwrap()
    }

    fn terms() -> TermsStore {
        let p = std::env::temp_dir().join(format!("loa-runner-terms-{}.json", std::process::id()));
        TermsStore::with_key(p, "k")
    }

    #[cfg(not(feature = "test-hooks"))]
    #[test]
    fn goto_outside_allowlist_is_blocked() {
        let sc = scenario(
            "channel.get",
            "steps:\n  - goto: \"https://evil.example/\"\n",
        );
        let mut b = FakeBrowser {
            current_url: "about:blank".into(),
            ..Default::default()
        };
        let t = terms();
        let r = Runner::new(
            &mut b,
            &NoHumanGate,
            &t,
            OpKind::ChannelGet,
            std::env::temp_dir(),
        );
        let e = r.run(&sc, &Vars::new()).err().unwrap();
        assert_eq!(e.code, Code::DomainBlocked);
        assert!(b.calls.iter().all(|c| c[0] != "open"));
    }

    #[cfg(not(feature = "test-hooks"))]
    #[test]
    fn login_page_without_human_returns_login_required() {
        let sc = scenario(
            "channel.get",
            "steps:\n  - goto: \"https://developers.line.biz/console/\"\n  - require_login: true\n",
        );
        let mut b = FakeBrowser {
            current_url: "https://account.line.biz/login?x=1".into(),
            ..Default::default()
        };
        let t = terms();
        let r = Runner::new(
            &mut b,
            &NoHumanGate,
            &t,
            OpKind::ChannelGet,
            std::env::temp_dir(),
        );
        let e = r.run(&sc, &Vars::new()).err().unwrap();
        assert_eq!(e.code, Code::LoginRequired);
    }

    #[cfg(not(feature = "test-hooks"))]
    #[test]
    fn fill_goes_through_find_with_exact() {
        let sc = scenario(
            "channel.create",
            "inputs:\n  name: { type: string, required: true }\nsteps:\n  - fill: { label: \"チャネル名\", value: \"{{name}}\" }\n",
        );
        let mut b = FakeBrowser {
            current_url: "https://developers.line.biz/console/".into(),
            ..Default::default()
        };
        let t = terms();
        let r = Runner::new(
            &mut b,
            &NoHumanGate,
            &t,
            OpKind::ChannelCreate,
            std::env::temp_dir(),
        );
        let vars: Vars = [("name".to_string(), "acme".to_string())]
            .into_iter()
            .collect();
        r.run(&sc, &vars).unwrap();
        assert!(b.calls.iter().any(|c| {
            c == &vec!["find", "label", "チャネル名", "fill", "acme", "--exact"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        }));
    }

    #[cfg(not(feature = "test-hooks"))]
    #[test]
    fn terms_denied_stops() {
        let sc = scenario(
            "channel.create",
            "steps:\n  - agree_terms: { terms_id: \"x\", title: \"規約\", label: \"同意する\" }\n",
        );
        let mut b = FakeBrowser {
            current_url: "https://developers.line.biz/console/".into(),
            ..Default::default()
        };
        let p =
            std::env::temp_dir().join(format!("loa-runner-terms-deny-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let t = TermsStore::with_key(p, "k");
        let r = Runner::new(
            &mut b,
            &NoHumanGate,
            &t,
            OpKind::ChannelCreate,
            std::env::temp_dir(),
        );
        let e = r.run(&sc, &Vars::new()).err().unwrap();
        assert_eq!(e.code, Code::ApprovalDenied);
        assert!(!b.calls.iter().any(|c| c.contains(&"check".to_string())));
    }

    #[test]
    fn resolve_scoped_rows() {
        let snap = Snapshot::parse(
            "- row \"acme\"\n  - button \"削除\" [ref=e5]\n- row \"beta\"\n  - button \"削除\" [ref=e6]",
        );
        let l = Locator {
            role: Some("button".into()),
            name: Some("削除".into()),
            within: Some(Box::new(Locator {
                role: Some("row".into()),
                name: Some("beta".into()),
                ..Default::default()
            })),
            ..Default::default()
        };
        assert_eq!(resolve_in(&snap, &l).unwrap(), vec!["e6".to_string()]);
    }

    #[test]
    fn extract_prefers_named_group() {
        assert_eq!(
            extract(
                r"/channel/(?<id>\d+)/",
                "https://x/console/channel/123/basics"
            )
            .as_deref(),
            Some("123")
        );
        assert_eq!(
            extract(r"(\d{10})", "ID 2001234567").as_deref(),
            Some("2001234567")
        );
    }
}
