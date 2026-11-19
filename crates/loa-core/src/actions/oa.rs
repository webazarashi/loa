//! LINE Official Accounts (LINE Official Account Manager, browser only).

use super::{Ctx, planned, vars};
use crate::error::{LoaError, Result};
use crate::gate::ApprovalRequest;
use crate::ops::{OpKind, PolicyContext};
use crate::output::Envelope;
use crate::resources::{ChannelRec, ChannelType, OaRec};
use crate::secrets::SecretRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CreateArgs {
    pub name: String,
    /// Industry shown in the entry form (大業種), e.g. "小売"
    #[serde(default)]
    pub category: Option<String>,
    /// 小業種
    #[serde(default)]
    pub subcategory: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UpdateArgs {
    pub oa: String,
    #[serde(default)]
    pub profile_name: Option<String>,
    #[serde(default)]
    pub profile_status_message: Option<String>,
    #[serde(default)]
    pub profile_icon: Option<String>,
    #[serde(default)]
    pub profile_background: Option<String>,
    #[serde(default)]
    pub settings_join_groups: Option<bool>,
    /// Issue an invitation for a new member (OAM adds members through invitation URLs).
    #[serde(default)]
    pub members_add: Option<String>,
    /// administrator | operator | operator_no_stats | operator_limited
    #[serde(default)]
    pub members_role: Option<String>,
    #[serde(default)]
    pub members_remove: Option<String>,
    /// chat | webhook | auto_reply
    #[serde(default)]
    pub response_mode: Option<String>,
}

const ROLES: [(&str, &str); 4] = [
    ("administrator", "管理者"),
    ("operator", "運用担当者"),
    ("operator_no_stats", "運用担当者（分析の閲覧権限なし）"),
    ("operator_limited", "運用担当者（配信権限なし）"),
];

fn role_label(r: &str) -> Result<&'static str> {
    ROLES
        .iter()
        .find(|(k, _)| *k == r)
        .map(|(_, v)| *v)
        .ok_or_else(|| LoaError::usage("--members.role は administrator / operator / operator_no_stats / operator_limited のいずれかです"))
}

/// (chat, webhook, auto_reply) for each response mode.
fn response_flags(mode: &str) -> Result<(bool, bool, bool)> {
    match mode {
        "chat" => Ok((true, false, false)),
        "webhook" => Ok((false, true, false)),
        "auto_reply" => Ok((false, false, true)),
        _ => Err(LoaError::usage(
            "--response.mode は chat / webhook / auto_reply のいずれかです",
        )),
    }
}

fn onoff(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

pub fn fetch(ctx: &mut Ctx<'_>) -> Result<Vec<(String, String)>> {
    let out = ctx.scenario("oa.list", OpKind::OaList, vars(&[]))?;
    let items: Vec<(String, String)> = out
        .values
        .get("accounts")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    Some((
                        v.get("id")?.as_str()?.to_string(),
                        v.get("name")?.as_str()?.trim().to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut res = ctx.resources()?;
    let biz = res.business(&ctx.business_id);
    for (id, name) in &items {
        biz.official_accounts.entry(name.clone()).or_default().id = id.clone();
    }
    ctx.save_resources(&res)?;
    Ok(items)
}

pub fn list(ctx: &mut Ctx<'_>) -> Result<Envelope> {
    let items = fetch(ctx)?;
    Ok(Envelope::new(
        "oa",
        "listed",
        Value::Array(
            items
                .iter()
                .map(|(id, n)| json!({"id": id, "name": n}))
                .collect(),
        ),
    ))
}

/// (name, id) of an official account by name or id.
pub fn resolve(ctx: &mut Ctx<'_>, key: &str) -> Result<(String, String)> {
    let lookup = |ctx: &Ctx<'_>| -> Result<Option<(String, String)>> {
        let res = ctx.resources()?;
        Ok(res.business_ref(&ctx.business_id).and_then(|b| {
            b.official_accounts
                .iter()
                .find(|(n, r)| n.as_str() == key || r.id == key)
                .map(|(n, r)| (n.clone(), r.id.clone()))
        }))
    };
    if let Some(hit) = lookup(ctx)? {
        return Ok(hit);
    }
    fetch(ctx)?;
    lookup(ctx)?.ok_or_else(|| {
        LoaError::usage(format!("公式アカウント {key} が見つかりません")).with_next("loa oa ls")
    })
}

pub fn get(ctx: &mut Ctx<'_>, key: &str) -> Result<Envelope> {
    let (name, id) = resolve(ctx, key)?;
    let out = ctx.scenario("oa.get", OpKind::OaGet, vars(&[("oa_id", &id)]))?;
    let mut data = json!({"id": id, "name": name});
    for k in ["basic_id", "status_message", "plan"] {
        if let Some(v) = out.str(k) {
            data[k] = json!(v);
        }
    }
    Ok(Envelope::new("oa", "got", data))
}

pub fn create(ctx: &mut Ctx<'_>, a: &CreateArgs) -> Result<Envelope> {
    if a.name.trim().is_empty() {
        return Err(LoaError::usage("--name を指定してください"));
    }
    if fetch(ctx)?.iter().any(|(_, n)| n == &a.name) {
        return Err(
            LoaError::usage(format!("公式アカウント {} は既にあります", a.name))
                .with_next(format!("loa oa get \"{}\"", a.name)),
        );
    }
    if ctx.dry_run {
        return Ok(planned("oa", json!({"create": a})));
    }
    let out = ctx.scenario(
        "oa.create",
        OpKind::OaCreate,
        vars(&[
            ("name", &a.name),
            ("category", a.category.as_deref().unwrap_or("")),
            ("subcategory", a.subcategory.as_deref().unwrap_or("")),
            ("company", a.company.as_deref().unwrap_or("")),
        ]),
    )?;
    let id = out.str("oa_id").unwrap_or_default();
    let mut res = ctx.resources()?;
    res.business(&ctx.business_id).official_accounts.insert(
        a.name.clone(),
        OaRec {
            id: id.clone(),
            ..Default::default()
        },
    );
    ctx.save_resources(&res)?;
    Ok(
        Envelope::new("oa", "created", json!({"id": id, "name": a.name})).with_next(format!(
            "loa oa enable-messaging-api \"{}\" --provider <provider>",
            a.name
        )),
    )
}

pub fn update(ctx: &mut Ctx<'_>, a: &UpdateArgs) -> Result<Envelope> {
    let any_profile = a.profile_name.is_some()
        || a.profile_status_message.is_some()
        || a.profile_icon.is_some()
        || a.profile_background.is_some();
    let any_members = a.members_add.is_some() || a.members_remove.is_some();
    if !any_profile && a.settings_join_groups.is_none() && !any_members && a.response_mode.is_none()
    {
        return Err(LoaError::usage(
            "変更する項目を指定してください（--profile.* / --settings.* / --members.* / --response.mode）",
        ));
    }
    if a.members_add.is_some() && a.members_remove.is_some() {
        return Err(LoaError::usage(
            "--members.add と --members.remove は同時に指定できません",
        ));
    }
    for f in [&a.profile_icon, &a.profile_background]
        .into_iter()
        .flatten()
    {
        let p = ctx.resolve_path(f);
        if !p.is_file() {
            return Err(LoaError::usage(format!(
                "画像ファイルがありません: {}",
                p.display()
            )));
        }
    }
    let (name, id) = resolve(ctx, &a.oa)?;
    let mut done = serde_json::Map::new();
    let mut plan = serde_json::Map::new();

    // Current response settings (shown as "before" in the plan and the approval dialog).
    let mut response_before = std::collections::BTreeMap::<String, String>::new();
    if a.response_mode.is_some() {
        let out = ctx.scenario("oa.get.response", OpKind::OaGet, vars(&[("oa_id", &id)]))?;
        for k in ["chat", "webhook", "auto_reply"] {
            if let Some(v) = out.str(k) {
                response_before.insert(k.into(), v);
            }
        }
    }
    // Ask every approval before changing anything, so a denial leaves the account untouched.
    if !ctx.dry_run {
        if let Some(who) = &a.members_add {
            let role = a.members_role.clone().unwrap_or_else(|| "operator".into());
            let label = role_label(&role)?;
            let req = ApprovalRequest::operation(
                OpKind::OaUpdateMembers,
                "公式アカウントにメンバーを招待します",
            )
            .target("公式アカウント", format!("{name} ({id})"))
            .change("メンバー", None, Some(format!("{who}（{label}）")))
            .note("発行される招待URLを受け取った人は、この権限で公式アカウントを操作できます。");
            ctx.approve(
                OpKind::OaUpdateMembers,
                &PolicyContext::default(),
                None,
                req,
            )?;
        }
        if let Some(who) = &a.members_remove {
            let req = ApprovalRequest::operation(
                OpKind::OaUpdateMembers,
                "公式アカウントからメンバーを削除します",
            )
            .target("公式アカウント", format!("{name} ({id})"))
            .change("メンバー", Some(who.clone()), Some("（削除）".into()));
            ctx.approve(
                OpKind::OaUpdateMembers,
                &PolicyContext::default(),
                None,
                req,
            )?;
        }
        if let Some(mode) = &a.response_mode {
            let (chat, webhook, auto) = response_flags(mode)?;
            let b = |k: &str| {
                response_before
                    .get(k)
                    .cloned()
                    .unwrap_or_else(|| "?".into())
            };
            let req = ApprovalRequest::operation(
                OpKind::OaUpdateResponse,
                "公式アカウントの応答設定を変更します",
            )
            .target("公式アカウント", format!("{name} ({id})"))
            .change("チャット", Some(b("chat")), Some(onoff(chat).into()))
            .change("Webhook", Some(b("webhook")), Some(onoff(webhook).into()))
            .change(
                "応答メッセージ",
                Some(b("auto_reply")),
                Some(onoff(auto).into()),
            )
            .note("本番の公式アカウントでは、友だちへの応答のしかたがすぐに変わります。");
            ctx.approve(
                OpKind::OaUpdateResponse,
                &PolicyContext::default(),
                None,
                req,
            )?;
        }
    }

    if any_profile {
        let icon = a
            .profile_icon
            .as_deref()
            .map(|p| ctx.resolve_path(p).display().to_string())
            .unwrap_or_default();
        let bg = a
            .profile_background
            .as_deref()
            .map(|p| ctx.resolve_path(p).display().to_string())
            .unwrap_or_default();
        let fields = json!({
            "name": a.profile_name, "status_message": a.profile_status_message,
            "icon": a.profile_icon, "background": a.profile_background,
        });
        if ctx.dry_run {
            plan.insert("profile".into(), fields);
        } else {
            ctx.scenario(
                "oa.update.profile",
                OpKind::OaUpdateProfile,
                vars(&[
                    ("oa_id", &id),
                    ("name", a.profile_name.as_deref().unwrap_or("")),
                    (
                        "status_message",
                        a.profile_status_message.as_deref().unwrap_or(""),
                    ),
                    ("icon", &icon),
                    ("background", &bg),
                ]),
            )?;
            done.insert("profile".into(), fields);
        }
    }
    if let Some(j) = a.settings_join_groups {
        if ctx.dry_run {
            plan.insert("settings".into(), json!({"join_groups": j}));
        } else {
            ctx.scenario(
                "oa.update.settings",
                OpKind::OaUpdateSettings,
                vars(&[("oa_id", &id), ("join_groups", onoff(j))]),
            )?;
            done.insert("settings".into(), json!({"join_groups": j}));
        }
    }
    if let Some(who) = &a.members_add {
        let role = a.members_role.clone().unwrap_or_else(|| "operator".into());
        let label = role_label(&role)?;
        if ctx.dry_run {
            plan.insert(
                "members".into(),
                json!({"invite": who, "role": role, "approval_required": true}),
            );
        } else {
            let out = ctx.scenario(
                "oa.update.members.invite",
                OpKind::OaUpdateMembers,
                vars(&[("oa_id", &id), ("role_label", label)]),
            )?;
            let invite = out.str("invite_url").unwrap_or_default();
            done.insert(
                "members".into(),
                json!({"invited": who, "role": role, "invite_url": invite}),
            );
        }
    }
    if let Some(who) = &a.members_remove {
        if ctx.dry_run {
            plan.insert(
                "members".into(),
                json!({"remove": who, "approval_required": true}),
            );
        } else {
            ctx.scenario(
                "oa.update.members.remove",
                OpKind::OaUpdateMembers,
                vars(&[("oa_id", &id), ("member", who)]),
            )?;
            done.insert("members".into(), json!({"removed": who}));
        }
    }
    if let Some(mode) = &a.response_mode {
        let (chat, webhook, auto) = response_flags(mode)?;
        let b = |k: &str| {
            response_before
                .get(k)
                .cloned()
                .unwrap_or_else(|| "?".into())
        };
        let after =
            json!({"chat": onoff(chat), "webhook": onoff(webhook), "auto_reply": onoff(auto)});
        if ctx.dry_run {
            plan.insert("response".into(), json!({"before": {"chat": b("chat"), "webhook": b("webhook"), "auto_reply": b("auto_reply")}, "after": after, "approval_required": true}));
        } else {
            ctx.scenario(
                "oa.update.response",
                OpKind::OaUpdateResponse,
                vars(&[
                    ("oa_id", &id),
                    ("name", &name),
                    ("chat", onoff(chat)),
                    ("webhook", onoff(webhook)),
                    ("auto_reply", onoff(auto)),
                ]),
            )?;
            done.insert("response".into(), after);
        }
    }
    if ctx.dry_run {
        return Ok(planned(
            "oa",
            json!({"update": {"id": id, "name": name, "changes": plan}}),
        ));
    }
    Ok(Envelope::new(
        "oa",
        "updated",
        json!({"id": id, "name": name, "changes": done}),
    ))
}

pub fn enable_messaging_api(ctx: &mut Ctx<'_>, key: &str, provider: &str) -> Result<Envelope> {
    super::provider::validate_name(provider)?;
    let (name, id) = resolve(ctx, key)?;
    if ctx.dry_run {
        return Ok(planned(
            "oa",
            json!({"enable_messaging_api": {"oa": name, "id": id, "provider": provider}, "approval_required": true}),
        ));
    }
    let req =
        ApprovalRequest::operation(OpKind::OaEnableMessagingApi, "Messaging APIを有効にします")
            .target("公式アカウント", format!("{name} ({id})"))
            .target("プロバイダー", provider)
            .change("Messaging API", Some("無効".into()), Some("有効".into()))
            .note("有効化は取り消せません。チャネルは後から別のプロバイダーへ移せません。");
    ctx.approve(
        OpKind::OaEnableMessagingApi,
        &PolicyContext::default(),
        Some(provider),
        req,
    )?;
    let out = ctx.scenario(
        "oa.enable_messaging_api",
        OpKind::OaEnableMessagingApi,
        vars(&[("oa_id", &id), ("name", &name), ("provider", provider)]),
    )?;
    let channel_id = out
        .str("channel_id")
        .ok_or_else(|| LoaError::ui_changed("Messaging APIチャネルのIDを読み取れませんでした"))?;
    let mut res = ctx.resources()?;
    let biz = res.business(&ctx.business_id);
    biz.upsert_channel(
        provider,
        "",
        &name,
        ChannelRec {
            id: channel_id.clone(),
            ty: ChannelType::Messaging,
            ..Default::default()
        },
    );
    let oa = biz.official_accounts.entry(name.clone()).or_default();
    oa.id = id.clone();
    oa.messaging_provider = Some(provider.to_string());
    oa.messaging_channel = Some(name.clone());
    ctx.save_resources(&res)?;
    let mut data =
        json!({"oa": name, "provider": provider, "channel_id": channel_id, "channel": name});
    if let Some(s) = out.secrets.get("channel_secret") {
        let r = SecretRef::new(provider, &name, "channel_secret");
        ctx.secrets.set(&r.store_key(), s)?;
        data["secret_ref"] = json!(r.uri());
    }
    Ok(Envelope::new("oa", "updated", data).with_next(format!(
        "loa webhook set --channel \"{name}\" --provider {provider} --url <URL>"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_modes() {
        assert_eq!(response_flags("webhook").unwrap(), (false, true, false));
        assert!(response_flags("bot").is_err());
    }

    #[test]
    fn roles() {
        assert_eq!(role_label("operator").unwrap(), "運用担当者");
        assert!(role_label("owner").is_err());
    }
}
