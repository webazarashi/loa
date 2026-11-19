//! `loa.yaml` with `plan` / `apply` (spec 5, v1.1: messaging channels live under
//! `official_accounts[].messaging_api`). Settings that cannot be read back cheaply are
//! compared with the values loa applied last time, which keeps `apply` idempotent.

use super::{Ctx, channel, liff, miniapp, oa, provider};
use crate::error::{LoaError, Result};
use crate::output::Envelope;
use crate::resources::ChannelType;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpecFile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_id: Option<String>,
    #[serde(default)]
    pub official_accounts: Vec<OaSpec>,
    #[serde(default)]
    pub providers: Vec<ProviderSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OaSpec {
    pub name: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub subcategory: Option<String>,
    #[serde(default)]
    pub profile: Option<ProfileSpec>,
    #[serde(default)]
    pub settings: Option<SettingsSpec>,
    #[serde(default)]
    pub response: Option<ResponseSpec>,
    #[serde(default)]
    pub messaging_api: Option<MessagingSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProfileSpec {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub background: Option<String>,
    #[serde(default)]
    pub status_message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SettingsSpec {
    #[serde(default)]
    pub join_groups: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResponseSpec {
    /// chat | webhook | auto_reply
    pub mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MessagingSpec {
    pub provider: String,
    #[serde(default)]
    pub webhook_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderSpec {
    pub name: String,
    #[serde(default)]
    pub channels: Vec<ChannelSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ChannelSpec {
    pub name: String,
    /// login | miniapp
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub callback_urls: Vec<String>,
    #[serde(default)]
    pub liff: Vec<LiffSpec>,
    #[serde(default)]
    pub miniapp: Option<MiniappSpec>,
    /// v1.0 field. Messaging channels are declared under official_accounts (v1.1).
    #[serde(default, skip_serializing)]
    pub webhook_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LiffSpec {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub size: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MiniappSpec {
    #[serde(default)]
    pub developing: Option<String>,
    #[serde(default)]
    pub review: Option<String>,
    #[serde(default)]
    pub published: Option<String>,
}

pub fn load(path: &Path) -> Result<SpecFile> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| LoaError::usage(format!("{} を読めません: {e}", path.display())))?;
    let spec: SpecFile = serde_saphyr::from_str(&text)
        .map_err(|e| LoaError::usage(format!("{} の形式が不正です: {e}", path.display())))?;
    validate(&spec)?;
    Ok(spec)
}

pub fn validate(spec: &SpecFile) -> Result<()> {
    if spec.version != 1 {
        return Err(LoaError::usage("loa.yaml の version は 1 にしてください"));
    }
    let mut names = BTreeSet::new();
    for p in &spec.providers {
        provider::validate_name(&p.name)?;
        for c in &p.channels {
            if c.ty == "messaging" || c.webhook_url.is_some() {
                return Err(LoaError::usage(format!(
                    "チャネル {} : Messaging APIチャネルは official_accounts[].messaging_api に書いてください（webhook_url もそこへ）",
                    c.name
                )));
            }
            if c.ty != "login" && c.ty != "miniapp" {
                return Err(LoaError::usage(format!(
                    "チャネル {} の type は login / miniapp のいずれかです",
                    c.name
                )));
            }
            crate::resources::validate_channel_name(&c.name)?;
            if !names.insert((p.name.clone(), c.name.clone())) {
                return Err(LoaError::usage(format!(
                    "チャネル {} が重複しています",
                    c.name
                )));
            }
            if !c.liff.is_empty() && c.ty != "login" {
                return Err(LoaError::usage(format!(
                    "チャネル {} : liff は login チャネルにだけ書けます",
                    c.name
                )));
            }
            if c.miniapp.is_some() && c.ty != "miniapp" {
                return Err(LoaError::usage(format!(
                    "チャネル {} : miniapp は miniapp チャネルにだけ書けます",
                    c.name
                )));
            }
        }
    }
    for o in &spec.official_accounts {
        if let Some(r) = &o.response
            && !matches!(r.mode.as_str(), "chat" | "webhook" | "auto_reply")
        {
            return Err(LoaError::usage(format!(
                "{} の response.mode は chat / webhook / auto_reply です",
                o.name
            )));
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
enum Act {
    ProviderCreate(String),
    OaCreate(oa::CreateArgs),
    OaEnable {
        oa: String,
        provider: String,
    },
    ChannelCreate(channel::CreateArgs),
    ChannelUpdate(channel::UpdateArgs),
    LiffEnsure(liff::AddArgs),
    LiffDelete {
        provider: String,
        channel: String,
        liff_id: String,
    },
    Miniapp(miniapp::UpdateArgs),
    Webhook {
        provider: String,
        channel: String,
        url: String,
    },
    OaUpdate(oa::UpdateArgs),
    ChannelDelete {
        provider: String,
        channel: String,
    },
}

#[derive(Debug, Clone)]
struct Item {
    op: &'static str,
    path: String,
    detail: Value,
    approval: bool,
    act: Act,
    /// (provider, channel) + field values to record after success, or OA name for OA fields.
    record: Record,
}

#[derive(Debug, Clone)]
enum Record {
    None,
    Channel {
        provider: String,
        channel: String,
        values: BTreeMap<String, String>,
    },
    Oa {
        name: String,
        values: BTreeMap<String, String>,
    },
}

fn file_digest(p: &Path) -> String {
    std::fs::read(p)
        .map(|b| hex::encode(&Sha256::digest(&b)[..8]))
        .unwrap_or_else(|_| "missing".into())
}

fn build(
    ctx: &mut Ctx<'_>,
    spec: &SpecFile,
    base: &Path,
    prune: bool,
) -> Result<(Vec<Item>, Vec<String>)> {
    let mut items = Vec::new();
    let mut notes = Vec::new();

    // ---- official accounts
    let existing_oas: BTreeSet<String> = if spec.official_accounts.is_empty() {
        BTreeSet::new()
    } else {
        oa::fetch(ctx)?.into_iter().map(|(_, n)| n).collect()
    };
    for o in &spec.official_accounts {
        let path = format!("official_accounts.{}", o.name);
        let exists = existing_oas.contains(&o.name);
        if !exists {
            items.push(Item {
                op: "add",
                path: path.clone(),
                detail: json!({"name": o.name, "note": "SMS認証など人の操作が入ります"}),
                approval: false,
                act: Act::OaCreate(oa::CreateArgs {
                    name: o.name.clone(),
                    category: o.category.clone(),
                    subcategory: o.subcategory.clone(),
                    company: None,
                }),
                record: Record::None,
            });
        }
        let res = ctx.resources()?;
        let rec = res
            .business_ref(&ctx.business_id)
            .and_then(|b| b.official_accounts.get(&o.name))
            .cloned()
            .unwrap_or_default();
        if let Some(m) = &o.messaging_api
            && rec.messaging_channel.is_none()
        {
            items.push(Item {
                op: "add",
                path: format!("{path}.messaging_api"),
                detail: json!({"provider": m.provider, "note": "有効化は取り消せません"}),
                approval: true,
                act: Act::OaEnable {
                    oa: o.name.clone(),
                    provider: m.provider.clone(),
                },
                record: Record::None,
            });
        }
        // profile / settings / response vs last applied values
        let mut up = oa::UpdateArgs {
            oa: o.name.clone(),
            ..Default::default()
        };
        let mut vals = BTreeMap::new();
        let mut changed = serde_json::Map::new();
        let mut cmp =
            |key: &str, want: String, changed: &mut serde_json::Map<String, Value>| -> bool {
                vals.insert(key.to_string(), want.clone());
                if rec.applied.get(key) != Some(&want) {
                    changed.insert(
                        key.into(),
                        json!({"before": rec.applied.get(key), "after": want}),
                    );
                    true
                } else {
                    false
                }
            };
        if let Some(p) = &o.profile {
            if let Some(n) = &p.name
                && cmp("profile.name", n.clone(), &mut changed)
            {
                up.profile_name = Some(n.clone());
            }
            if let Some(s) = &p.status_message
                && cmp("profile.status_message", s.clone(), &mut changed)
            {
                up.profile_status_message = Some(s.clone());
            }
            if let Some(i) = &p.icon {
                let full = base.join(i);
                if cmp("profile.icon", file_digest(&full), &mut changed) {
                    up.profile_icon = Some(full.display().to_string());
                }
            }
            if let Some(i) = &p.background {
                let full = base.join(i);
                if cmp("profile.background", file_digest(&full), &mut changed) {
                    up.profile_background = Some(full.display().to_string());
                }
            }
        }
        if let Some(s) = &o.settings
            && let Some(j) = s.join_groups
            && cmp("settings.join_groups", j.to_string(), &mut changed)
        {
            up.settings_join_groups = Some(j);
        }
        let mut response_approval = false;
        if let Some(r) = &o.response
            && cmp("response.mode", r.mode.clone(), &mut changed)
        {
            up.response_mode = Some(r.mode.clone());
            response_approval = true;
        }
        if !changed.is_empty() {
            items.push(Item {
                op: "change",
                path: path.clone(),
                detail: Value::Object(changed),
                approval: response_approval,
                act: Act::OaUpdate(up),
                record: Record::Oa {
                    name: o.name.clone(),
                    values: vals,
                },
            });
        }
        if let Some(m) = &o.messaging_api
            && let Some(url) = &m.webhook_url
        {
            let mut current: Option<String> = None;
            if rec.messaging_channel.is_some()
                && let Ok(hit) = ctx.find_channel(&o.name, Some(&m.provider))
                && let Ok(token) = ctx.channel_token(&hit)
            {
                current = ctx.api()?.webhook_get(&token).ok().map(|w| w.endpoint);
            }
            match current {
                Some(c) if &c == url => {}
                other => {
                    let approval = other.as_deref().is_some_and(|c| !c.is_empty());
                    items.push(Item {
                        op: if other.is_some() { "change" } else { "add" },
                        path: format!("{path}.messaging_api.webhook_url"),
                        detail: json!({"before": other, "after": url}),
                        approval,
                        act: Act::Webhook {
                            provider: m.provider.clone(),
                            channel: o.name.clone(),
                            url: url.clone(),
                        },
                        record: Record::None,
                    });
                }
            }
        }
    }

    // ---- providers and channels
    let existing_providers: BTreeSet<String> = if spec.providers.is_empty() {
        BTreeSet::new()
    } else {
        provider::fetch(ctx)?.into_iter().map(|p| p.name).collect()
    };
    for p in &spec.providers {
        let ppath = format!("providers.{}", p.name);
        let provider_exists = existing_providers.contains(&p.name);
        if !provider_exists {
            items.push(Item {
                op: "add",
                path: ppath.clone(),
                detail: json!({"name": p.name}),
                approval: false,
                act: Act::ProviderCreate(p.name.clone()),
                record: Record::None,
            });
        }
        let existing_channels: BTreeSet<String> = if provider_exists {
            channel::fetch(ctx, &p.name)?;
            ctx.resources()?
                .business_ref(&ctx.business_id)
                .and_then(|b| b.providers.get(&p.name))
                .map(|pr| pr.channels.keys().cloned().collect())
                .unwrap_or_default()
        } else {
            BTreeSet::new()
        };
        for c in &p.channels {
            let cpath = format!("{ppath}.channels.{}", c.name);
            let mut vals = BTreeMap::new();
            if let Some(d) = &c.description {
                vals.insert("description".to_string(), d.clone());
            }
            if !c.callback_urls.is_empty() {
                vals.insert("callback_urls".to_string(), c.callback_urls.join(" "));
            }
            let exists = existing_channels.contains(&c.name);
            if !exists {
                items.push(Item {
                    op: "add",
                    path: cpath.clone(),
                    detail: json!({"type": c.ty, "description": c.description, "callback_urls": c.callback_urls}),
                    approval: false,
                    act: Act::ChannelCreate(channel::CreateArgs {
                        provider: p.name.clone(),
                        ty: c.ty.clone(),
                        name: c.name.clone(),
                        description: c.description.clone(),
                        email: None,
                        callback_urls: c.callback_urls.clone(),
                    }),
                    record: Record::Channel { provider: p.name.clone(), channel: c.name.clone(), values: vals.clone() },
                });
            } else {
                let rec = ctx
                    .resources()?
                    .business_ref(&ctx.business_id)
                    .and_then(|b| b.providers.get(&p.name))
                    .and_then(|pr| pr.channels.get(&c.name))
                    .cloned()
                    .unwrap_or_default();
                let mut up = channel::UpdateArgs {
                    channel: c.name.clone(),
                    provider: Some(p.name.clone()),
                    ..Default::default()
                };
                let mut changed = serde_json::Map::new();
                if let Some(d) = &c.description
                    && rec.applied.get("description") != Some(d)
                {
                    up.description = Some(d.clone());
                    changed.insert(
                        "description".into(),
                        json!({"before": rec.applied.get("description"), "after": d}),
                    );
                }
                let cb = c.callback_urls.join(" ");
                if !c.callback_urls.is_empty() && rec.applied.get("callback_urls") != Some(&cb) {
                    up.callback_urls = c.callback_urls.clone();
                    changed.insert(
                        "callback_urls".into(),
                        json!({"before": rec.applied.get("callback_urls"), "after": cb}),
                    );
                }
                if !changed.is_empty() {
                    items.push(Item {
                        op: "change",
                        path: cpath.clone(),
                        detail: Value::Object(changed),
                        approval: false,
                        act: Act::ChannelUpdate(up),
                        record: Record::Channel {
                            provider: p.name.clone(),
                            channel: c.name.clone(),
                            values: vals.clone(),
                        },
                    });
                }
            }
            // LIFF
            if c.ty == "login" {
                let mut current: Option<Vec<crate::line_api::LiffApp>> = None;
                if exists && let Ok(hit) = ctx.find_channel(&c.name, Some(&p.name)) {
                    match liff::fetch(ctx, &hit) {
                        Ok(apps) => current = Some(apps),
                        Err(e) => notes.push(format!(
                            "{cpath}: LIFFの現状を取得できません（{}）",
                            e.message
                        )),
                    }
                }
                for l in &c.liff {
                    let size = l.size.clone().unwrap_or_else(|| "full".into());
                    let found = current.as_ref().and_then(|apps| {
                        apps.iter()
                            .find(|a| a.description.as_deref() == Some(l.name.as_str()))
                    });
                    let (op, detail) = match found {
                        Some(a) if a.view.url == l.url && a.view.ty == size => continue,
                        Some(a) => (
                            "change",
                            json!({"liff_id": a.liff_id, "url": {"before": a.view.url, "after": l.url}, "size": {"before": a.view.ty, "after": size}}),
                        ),
                        None if current.is_none() && exists => (
                            "ensure",
                            json!({"url": l.url, "size": size, "note": "現状を確認できないため ensure で反映します"}),
                        ),
                        None => ("add", json!({"url": l.url, "size": size})),
                    };
                    items.push(Item {
                        op,
                        path: format!("{cpath}.liff.{}", l.name),
                        detail,
                        approval: false,
                        act: Act::LiffEnsure(liff::AddArgs {
                            channel: c.name.clone(),
                            provider: Some(p.name.clone()),
                            name: l.name.clone(),
                            url: l.url.clone(),
                            size: Some(size),
                            scope: None,
                            bot_prompt: None,
                        }),
                        record: Record::None,
                    });
                }
                if prune && let Some(apps) = &current {
                    let wanted: BTreeSet<&str> = c.liff.iter().map(|l| l.name.as_str()).collect();
                    for a in apps {
                        if !a.description.as_deref().is_some_and(|d| wanted.contains(d)) {
                            items.push(Item {
                                op: "delete",
                                path: format!(
                                    "{cpath}.liff.{}",
                                    a.description.clone().unwrap_or_else(|| a.liff_id.clone())
                                ),
                                detail: json!({"liff_id": a.liff_id, "url": a.view.url}),
                                approval: true,
                                act: Act::LiffDelete {
                                    provider: p.name.clone(),
                                    channel: c.name.clone(),
                                    liff_id: a.liff_id.clone(),
                                },
                                record: Record::None,
                            });
                        }
                    }
                }
            }
            // MINI App endpoints
            if let Some(m) = &c.miniapp {
                let rec = ctx
                    .resources()?
                    .business_ref(&ctx.business_id)
                    .and_then(|b| b.providers.get(&p.name))
                    .and_then(|pr| pr.channels.get(&c.name))
                    .cloned()
                    .unwrap_or_default();
                for (stage, url) in [
                    ("developing", &m.developing),
                    ("review", &m.review),
                    ("published", &m.published),
                ] {
                    let Some(url) = url else { continue };
                    let key = format!("endpoint.{stage}");
                    if rec.applied.get(&key) == Some(url) {
                        continue;
                    }
                    let mut v = BTreeMap::new();
                    v.insert(key.clone(), url.clone());
                    items.push(Item {
                        op: "change",
                        path: format!("{cpath}.miniapp.{stage}"),
                        detail: json!({"before": rec.applied.get(&key), "after": url}),
                        approval: false,
                        act: Act::Miniapp(miniapp::UpdateArgs {
                            channel: c.name.clone(),
                            provider: Some(p.name.clone()),
                            stage: stage.into(),
                            endpoint_url: url.clone(),
                        }),
                        record: Record::Channel {
                            provider: p.name.clone(),
                            channel: c.name.clone(),
                            values: v,
                        },
                    });
                }
            }
        }
        if prune && provider_exists {
            let wanted: BTreeSet<&str> = p.channels.iter().map(|c| c.name.as_str()).collect();
            let messaging: BTreeSet<String> = spec
                .official_accounts
                .iter()
                .filter(|o| {
                    o.messaging_api
                        .as_ref()
                        .is_some_and(|m| m.provider == p.name)
                })
                .map(|o| o.name.clone())
                .collect();
            let res = ctx.resources()?;
            if let Some(pr) = res
                .business_ref(&ctx.business_id)
                .and_then(|b| b.providers.get(&p.name))
            {
                for (name, rec) in &pr.channels {
                    if wanted.contains(name.as_str())
                        || messaging.contains(name)
                        || rec.ty == ChannelType::Messaging
                    {
                        continue;
                    }
                    items.push(Item {
                        op: "delete",
                        path: format!("{ppath}.channels.{name}"),
                        detail: json!({"id": rec.id, "type": rec.ty.as_str()}),
                        approval: true,
                        act: Act::ChannelDelete {
                            provider: p.name.clone(),
                            channel: name.clone(),
                        },
                        record: Record::None,
                    });
                }
            }
        }
    }
    // apply order: creates first, deletes last
    let rank = |i: &Item| match i.act {
        Act::ProviderCreate(_) => 0,
        Act::OaCreate(_) => 1,
        Act::OaEnable { .. } => 2,
        Act::ChannelCreate(_) => 3,
        Act::ChannelUpdate(_) => 4,
        Act::LiffEnsure(_) => 5,
        Act::Miniapp(_) => 6,
        Act::Webhook { .. } => 7,
        Act::OaUpdate(_) => 8,
        Act::LiffDelete { .. } => 9,
        Act::ChannelDelete { .. } => 10,
    };
    items.sort_by_key(rank);
    Ok((items, notes))
}

fn item_json(i: &Item) -> Value {
    json!({"op": i.op, "path": i.path, "detail": i.detail, "approval_required": i.approval})
}

fn summary(items: &[Item]) -> Value {
    let count = |op: &str| items.iter().filter(|i| i.op == op).count();
    json!({
        "add": count("add") + count("ensure"),
        "change": count("change"),
        "delete": count("delete"),
        "approvals": items.iter().filter(|i| i.approval).count(),
    })
}

fn spec_path(ctx: &Ctx<'_>, file: &str) -> (PathBuf, PathBuf) {
    let p = ctx.resolve_path(file);
    let base = p.parent().map(Path::to_path_buf).unwrap_or_default();
    (p, base)
}

fn check_business(ctx: &Ctx<'_>, spec: &SpecFile) -> Result<()> {
    if let Some(b) = &spec.business_id
        && b != &ctx.business_id
    {
        return Err(LoaError::usage(format!(
            "loa.yaml は business_id {b} 用です。--business-id {b} を付けて実行してください"
        )));
    }
    Ok(())
}

pub fn plan(ctx: &mut Ctx<'_>, file: &str) -> Result<Envelope> {
    let (path, base) = spec_path(ctx, file);
    let spec = load(&path)?;
    check_business(ctx, &spec)?;
    let was = ctx.dry_run;
    ctx.dry_run = true;
    let built = build(ctx, &spec, &base, false);
    ctx.dry_run = was;
    let (items, notes) = built?;
    let mut env = Envelope::new(
        "plan",
        if items.is_empty() {
            "unchanged"
        } else {
            "planned"
        },
        json!({"file": path.display().to_string(), "changes": items.iter().map(item_json).collect::<Vec<_>>(), "summary": summary(&items)}),
    );
    env.warnings.extend(notes);
    if !items.is_empty() {
        env = env.with_next(format!("loa apply -f {file}"));
    }
    Ok(env)
}

pub fn apply(ctx: &mut Ctx<'_>, file: &str, prune: bool) -> Result<Envelope> {
    let (path, base) = spec_path(ctx, file);
    let spec = load(&path)?;
    check_business(ctx, &spec)?;
    let was = ctx.dry_run;
    ctx.dry_run = true;
    let built = build(ctx, &spec, &base, prune);
    ctx.dry_run = was;
    let (items, notes) = built?;
    if ctx.dry_run {
        return plan(ctx, file);
    }
    let mut done: Vec<Value> = Vec::new();
    for item in &items {
        ctx.gate
            .progress(&format!("apply: {} {}", item.op, item.path));
        let r = match &item.act {
            Act::ProviderCreate(n) => provider::create(ctx, n, true),
            Act::OaCreate(a) => oa::create(ctx, a),
            Act::OaEnable { oa: o, provider: p } => oa::enable_messaging_api(ctx, o, p),
            Act::ChannelCreate(a) => channel::create(ctx, a, true),
            Act::ChannelUpdate(a) => channel::update(ctx, a),
            Act::LiffEnsure(a) => liff::add(ctx, a, true),
            Act::LiffDelete {
                provider: p,
                channel: c,
                liff_id,
            } => liff::remove(ctx, c, Some(p), liff_id),
            Act::Miniapp(a) => miniapp::update(ctx, a),
            Act::Webhook {
                provider: p,
                channel: c,
                url,
            } => super::webhook::set(ctx, c, Some(p), url),
            Act::OaUpdate(a) => oa::update(ctx, a),
            Act::ChannelDelete {
                provider: p,
                channel: c,
            } => channel::remove(ctx, c, Some(p)),
        };
        match r {
            Ok(env) => {
                record(ctx, &item.record)?;
                done.push(json!({"op": item.op, "path": item.path, "result": env.action}));
            }
            Err(mut e) => {
                e.detail = Some(json!({
                    "failed_at": item.path,
                    "applied": done,
                    "note": "もう一度 loa apply を実行すると、済んだ変更は飛ばして続きから進みます",
                }));
                return Err(e);
            }
        }
    }
    let mut env = Envelope::new(
        "apply",
        if items.is_empty() {
            "unchanged"
        } else {
            "applied"
        },
        json!({"file": path.display().to_string(), "applied": done, "summary": summary(&items)}),
    );
    env.warnings.extend(notes);
    Ok(env)
}

fn record(ctx: &Ctx<'_>, r: &Record) -> Result<()> {
    let mut res = ctx.resources()?;
    match r {
        Record::None => return Ok(()),
        Record::Channel {
            provider,
            channel,
            values,
        } => {
            if let Some(c) = res
                .business(&ctx.business_id)
                .channel_mut(provider, channel)
            {
                c.applied.extend(values.clone());
            }
        }
        Record::Oa { name, values } => {
            res.business(&ctx.business_id)
                .official_accounts
                .entry(name.clone())
                .or_default()
                .applied
                .extend(values.clone());
        }
    }
    ctx.save_resources(&res)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: &str = r#"
version: 1
business_id: "acme"
official_accounts:
  - name: "Acme Store"
    profile:
      icon: "./assets/icon.png"
      status_message: "ご予約はこちらから"
    settings:
      join_groups: false
    response:
      mode: webhook
    messaging_api:
      provider: "acme"
      webhook_url: "https://api.example.com/webhook"
providers:
  - name: "acme"
    channels:
      - name: "acme-liff-stg"
        type: login
        callback_urls: ["https://stg.example.com/cb"]
        liff:
          - name: "main"
            url: "https://stg.example.com"
            size: full
      - name: "acme-mini"
        type: miniapp
        miniapp:
          developing: "https://dev.example.com"
"#;

    #[test]
    fn parses_v11_example() {
        let s: SpecFile = serde_saphyr::from_str(SPEC).unwrap();
        validate(&s).unwrap();
        assert_eq!(s.providers[0].channels.len(), 2);
        assert_eq!(
            s.official_accounts[0]
                .messaging_api
                .as_ref()
                .unwrap()
                .webhook_url
                .as_deref(),
            Some("https://api.example.com/webhook")
        );
    }

    #[test]
    fn v10_messaging_channel_is_rejected_with_guidance() {
        let y = "version: 1\nproviders:\n  - name: acme\n    channels:\n      - name: acme-bot\n        type: messaging\n        webhook_url: \"https://x\"\n";
        let s: SpecFile = serde_saphyr::from_str(y).unwrap();
        let e = validate(&s).unwrap_err();
        assert!(e.message.contains("official_accounts"));
    }

    #[test]
    fn liff_only_on_login() {
        let y = "version: 1\nproviders:\n  - name: acme\n    channels:\n      - name: m\n        type: miniapp\n        liff:\n          - { name: a, url: \"https://x\" }\n";
        let s: SpecFile = serde_saphyr::from_str(y).unwrap();
        assert!(validate(&s).is_err());
    }
}
