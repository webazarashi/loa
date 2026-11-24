//! channel ls / get / create / ensure / update / rm

use super::{Ctx, KEY_ACCESS_TOKEN, KEY_CHANNEL_SECRET, planned, provider, vars};
use crate::error::{LoaError, Result};
use crate::gate::ApprovalRequest;
use crate::ops::{OpKind, PolicyContext};
use crate::output::Envelope;
use crate::resources::{ChannelHit, ChannelRec, ChannelType, validate_channel_name};
use crate::secrets::SecretRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct CreateArgs {
    pub provider: String,
    /// login | miniapp (messaging channels come from `oa enable-messaging-api`)
    #[serde(rename = "type")]
    pub ty: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    /// LINE Login callback URLs (login channels)
    #[serde(default)]
    pub callback_urls: Vec<String>,
    /// Official account linked through the friendship option (login channels), by name or @id
    #[serde(default)]
    pub linked_oa: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UpdateArgs {
    pub channel: String,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub callback_urls: Vec<String>,
    /// Official account linked through the friendship option (login channels), by name or @id
    #[serde(default)]
    pub linked_oa: Option<String>,
}

fn parse_type(s: &str) -> Result<ChannelType> {
    match ChannelType::parse(s) {
        Some(ChannelType::Messaging) => Err(LoaError::usage(
            "Messaging APIチャネルはLINE Developersコンソールから作成できません（2024年9月以降）。公式アカウントでMessaging APIを有効にすると作成されます",
        )
        .with_next("loa oa enable-messaging-api <oa> --provider <provider>")),
        Some(t) => Ok(t),
        None => Err(LoaError::usage("--type は login / miniapp のいずれかです")),
    }
}

/// A channel card on the provider page.
#[derive(Debug, Clone, PartialEq)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub ty: ChannelType,
}

pub struct Listing {
    pub provider: String,
    pub provider_id: String,
    pub cards: Vec<Card>,
}

/// Channels of a provider from the console; refreshes the local map.
pub fn fetch(ctx: &mut Ctx<'_>, provider_key: &str) -> Result<Listing> {
    let (pname, pid) = provider::resolve_id(ctx, provider_key)?;
    let out = ctx.scenario(
        "channel.list",
        OpKind::ChannelList,
        vars(&[("provider_id", &pid)]),
    )?;
    let cards: Vec<Card> = out
        .values
        .get("channels")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(card)
                .filter(|c| !c.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut res = ctx.resources()?;
    let biz = res.business(&ctx.business_id);
    let prov = biz.providers.entry(pname.clone()).or_default();
    prov.id = pid.clone();
    for c in &cards {
        prov.put_channel(
            &c.name,
            ChannelRec {
                id: c.id.clone(),
                ty: c.ty,
                ..Default::default()
            },
        );
    }
    // Channels deleted in the console disappear from the map. An empty listing is not trusted
    // (a broken list scenario would otherwise make `ensure` create duplicates).
    let stale_kept = if cards.is_empty() {
        !prov.channels.is_empty()
    } else {
        let live: std::collections::HashSet<&str> = cards.iter().map(|c| c.id.as_str()).collect();
        prov.channels
            .retain(|_, c| c.id.is_empty() || live.contains(c.id.as_str()));
        prov.reindex();
        false
    };
    ctx.save_resources(&res)?;
    if stale_kept {
        ctx.warnings.push(format!(
            "コンソールでプロバイダー {pname} のチャネルを読み取れませんでした。手元の対応表のまま進めます"
        ));
    }
    Ok(Listing {
        provider: pname,
        provider_id: pid,
        cards,
    })
}

const TYPE_LABELS: [(&str, ChannelType); 6] = [
    ("Messaging API", ChannelType::Messaging),
    ("LINEログイン", ChannelType::Login),
    ("LINE Login", ChannelType::Login),
    ("LINEミニアプリ", ChannelType::Miniapp),
    ("LINE MINI App", ChannelType::Miniapp),
    ("ミニアプリ", ChannelType::Miniapp),
];

/// One channel card from `read_list`. The console card holds the name in a heading and the type
/// as a separate badge among `parts`.
fn card(v: &Value) -> Option<Card> {
    let id = v.get("id")?.as_str()?.to_string();
    let name = v.get("name")?.as_str()?.trim();
    let badge = v
        .get("parts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find_map(|p| {
            TYPE_LABELS
                .iter()
                .find(|(label, _)| p.trim() == *label)
                .map(|(_, ty)| *ty)
        });
    // Without a heading the name is the whole link text, which also holds the type label.
    let from_heading = v.get("heading").and_then(Value::as_bool).unwrap_or(false);
    let (name, split_ty) = if from_heading {
        (name.to_string(), ChannelType::Unknown)
    } else {
        split_type(name)
    };
    Some(Card {
        id,
        name,
        ty: badge.unwrap_or(split_ty),
    })
}

/// Older cards put the type in the same text as the name; strip it if present.
fn split_type(raw: &str) -> (String, ChannelType) {
    let s = raw.trim();
    for (label, ty) in TYPE_LABELS {
        if let Some(rest) = s.strip_suffix(label) {
            return (rest.trim().to_string(), ty);
        }
        if let Some(rest) = s.strip_prefix(label) {
            return (rest.trim().to_string(), ty);
        }
    }
    (s.to_string(), ChannelType::Unknown)
}

pub fn list(ctx: &mut Ctx<'_>, provider_key: &str) -> Result<Envelope> {
    let l = fetch(ctx, provider_key)?;
    if l.cards.is_empty() {
        // Listing unreadable: fall back to the local map (fetch already warned).
        let res = ctx.resources()?;
        let chans: Vec<Value> = res
            .business_ref(&ctx.business_id)
            .and_then(|b| b.providers.get(&l.provider))
            .map(|p| {
                p.channels
                    .iter()
                    .map(|(n, c)| {
                        let name = c.display_name.as_deref().unwrap_or(n);
                        json!({"id": c.id, "name": name, "type": c.ty.as_str()})
                    })
                    .collect()
            })
            .unwrap_or_default();
        return Ok(Envelope::new("channel", "listed", Value::Array(chans)));
    }
    let res = ctx.resources()?;
    if let Some(p) = res
        .business_ref(&ctx.business_id)
        .and_then(|b| b.providers.get(&l.provider))
    {
        for (name, ids) in &p.ambiguous {
            ctx.warnings.push(format!(
                "チャネル名 {name} は複数あります（ID: {}）。これらのチャネルは ID で指定してください",
                ids.join(", ")
            ));
        }
    }
    let chans = l
        .cards
        .iter()
        .map(|c| json!({"id": c.id, "name": c.name, "type": c.ty.as_str()}))
        .collect();
    Ok(Envelope::new("channel", "listed", Value::Array(chans)))
}

/// Find a channel; if unknown locally and a provider is given, refresh that provider first.
pub fn resolve(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<ChannelHit> {
    match ctx.find_channel(key, provider) {
        Ok(h) => Ok(h),
        Err(e) => {
            if let Some(p) = provider {
                fetch(ctx, p)?;
                return ctx.find_channel(key, provider);
            }
            Err(e)
        }
    }
}

/// Store a channel-scoped secret (keyed by channel id) and return its `loa://` reference.
pub(crate) fn store_secret(
    ctx: &Ctx<'_>,
    provider: &str,
    channel_id: &str,
    key: &str,
    value: &str,
) -> Result<String> {
    let r = SecretRef::new(provider, channel_id, key);
    ctx.secrets.set(&r.store_key(), value)?;
    Ok(r.uri())
}

pub fn get(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<Envelope> {
    let hit = resolve(ctx, key, provider)?;
    let out = ctx.scenario(
        "channel.get",
        OpKind::ChannelGet,
        vars(&[("channel_id", &hit.rec.id)]),
    )?;
    // The console's name is the truth; the local map may predate a rename or a shared name.
    let name = out
        .str("name")
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| hit.name.clone());
    if name != hit.name {
        let mut res = ctx.resources()?;
        res.business(&ctx.business_id)
            .put_channel(&hit.provider, "", &name, hit.rec.clone());
        ctx.save_resources(&res)?;
    }
    let mut data = json!({
        "id": hit.rec.id,
        "name": name,
        "provider": hit.provider,
        "type": hit.rec.ty.as_str(),
        "description": out.str("description"),
    });
    if let Some(s) = out.secrets.get(KEY_CHANNEL_SECRET) {
        data["secret_ref"] = json!(store_secret(
            ctx,
            &hit.provider,
            &hit.rec.id,
            KEY_CHANNEL_SECRET,
            s
        )?);
    }
    Ok(Envelope::new("channel", "got", data))
}

pub fn create(ctx: &mut Ctx<'_>, a: &CreateArgs, ensure: bool) -> Result<Envelope> {
    let ty = parse_type(&a.ty)?;
    validate_channel_name(&a.name)?;
    for u in &a.callback_urls {
        if !u.starts_with("https://") {
            return Err(LoaError::usage(format!(
                "コールバックURLは https:// で始めてください: {u}"
            )));
        }
    }
    let l = fetch(ctx, &a.provider)?;
    let (pname, pid) = (l.provider, l.provider_id);
    // Ambiguous names are an error here: `ensure` must not add yet another channel of that name.
    let found = ctx
        .resources()?
        .business_ref(&ctx.business_id)
        .map(|b| b.find_channel(&a.name, Some(&pname)))
        .transpose()?
        .flatten();
    if let Some(hit) = found {
        if !ensure {
            return Err(LoaError::usage(format!(
                "チャネル {} は既にあります（ID {}）",
                a.name, hit.rec.id
            ))
            .with_next(format!("loa channel get {} --provider {pname}", a.name)));
        }
        // ensure: apply differences that can be compared locally (description / callback urls)
        let wants_update = a.description.is_some()
            || !a.callback_urls.is_empty()
            || a.email.is_some()
            || a.linked_oa.is_some();
        if !wants_update {
            return Ok(Envelope::new(
                "channel",
                "unchanged",
                json!({"id": hit.rec.id, "name": hit.name, "provider": pname, "type": hit.rec.ty.as_str()}),
            ));
        }
        return update(
            ctx,
            &UpdateArgs {
                channel: a.name.clone(),
                provider: Some(pname),
                name: None,
                description: a.description.clone(),
                email: a.email.clone(),
                callback_urls: a.callback_urls.clone(),
                linked_oa: a.linked_oa.clone(),
            },
        );
    }
    if ctx.dry_run {
        return Ok(planned(
            "channel",
            json!({"create": {"provider": pname, "type": ty.as_str(), "name": a.name}}),
        ));
    }
    let scenario = match ty {
        ChannelType::Login => "channel.create.login",
        ChannelType::Miniapp => "channel.create.miniapp",
        _ => unreachable!(),
    };
    let desc = a.description.clone().unwrap_or_else(|| a.name.clone());
    let email = a.email.clone().unwrap_or_default();
    let out = ctx.scenario(
        scenario,
        OpKind::ChannelCreate,
        vars(&[
            ("provider_id", &pid),
            ("name", &a.name),
            ("description", &desc),
            ("email", &email),
        ]),
    )?;
    let id = out
        .str("channel_id")
        .ok_or_else(|| LoaError::ui_changed("作成したチャネルのIDを読み取れませんでした"))?;
    let mut res = ctx.resources()?;
    res.business(&ctx.business_id).put_channel(
        &pname,
        &pid,
        &a.name,
        ChannelRec {
            id: id.clone(),
            ty,
            ..Default::default()
        },
    );
    ctx.save_resources(&res)?;
    let mut data = json!({"id": id, "name": a.name, "type": ty.as_str(), "provider": pname});
    if let Some(s) = out.secrets.get(KEY_CHANNEL_SECRET) {
        data["secret_ref"] = json!(store_secret(ctx, &pname, &id, KEY_CHANNEL_SECRET, s)?);
    }
    let mut env = Envelope::new("channel", "created", data);
    if ty == ChannelType::Login {
        if !a.callback_urls.is_empty() || a.linked_oa.is_some() {
            let u = update(
                ctx,
                &UpdateArgs {
                    channel: id.clone(),
                    provider: Some(pname.clone()),
                    callback_urls: a.callback_urls.clone(),
                    linked_oa: a.linked_oa.clone(),
                    ..Default::default()
                },
            )?;
            env.warnings.extend(u.warnings);
        }
        env = env.with_next(format!(
            "loa liff ensure --channel {} --name main --url <URL> --size full",
            a.name
        ));
    } else {
        env = env.with_next(format!("loa miniapp get {}", a.name));
    }
    Ok(env)
}

pub fn update(ctx: &mut Ctx<'_>, a: &UpdateArgs) -> Result<Envelope> {
    let hit = resolve(ctx, &a.channel, a.provider.as_deref())?;
    if let Some(n) = &a.name {
        validate_channel_name(n)?;
    }
    let mut changes = serde_json::Map::new();
    if let Some(n) = &a.name {
        changes.insert("name".into(), json!(n));
    }
    if let Some(d) = &a.description {
        changes.insert("description".into(), json!(d));
    }
    if let Some(e) = &a.email {
        changes.insert("email".into(), json!(e));
    }
    if !a.callback_urls.is_empty() {
        if hit.rec.ty == ChannelType::Miniapp || hit.rec.ty == ChannelType::Messaging {
            return Err(LoaError::usage(
                "コールバックURLを設定できるのは LINEログインチャネルだけです",
            ));
        }
        for u in &a.callback_urls {
            if !u.starts_with("https://") {
                return Err(LoaError::usage(format!(
                    "コールバックURLは https:// で始めてください: {u}"
                )));
            }
        }
        changes.insert("callback_urls".into(), json!(a.callback_urls));
    }
    let linked = match &a.linked_oa {
        Some(_) if hit.rec.ty == ChannelType::Miniapp || hit.rec.ty == ChannelType::Messaging => {
            return Err(LoaError::usage(
                "公式アカウントをリンクできるのは LINEログインチャネルだけです",
            ));
        }
        Some(key) => {
            let (oa_name, oa_id) = super::oa::resolve(ctx, key)?;
            changes.insert("linked_oa".into(), json!(oa_id));
            Some((oa_name, oa_id))
        }
        None => None,
    };
    if changes.is_empty() {
        return Err(LoaError::usage(
            "変更する項目を指定してください（--name / --description / --email / --callback-url / --linked-oa）",
        ));
    }
    if ctx.dry_run {
        return Ok(planned(
            "channel",
            json!({"update": {"id": hit.rec.id, "name": hit.name, "changes": changes}}),
        ));
    }
    if a.name.is_some() || a.description.is_some() || a.email.is_some() {
        let name = a.name.clone().unwrap_or_default();
        let desc = a.description.clone().unwrap_or_default();
        let email = a.email.clone().unwrap_or_default();
        ctx.scenario(
            "channel.update.basic",
            OpKind::ChannelUpdate,
            vars(&[
                ("channel_id", &hit.rec.id),
                ("name", &name),
                ("description", &desc),
                ("email", &email),
            ]),
        )?;
    }
    if !a.callback_urls.is_empty() {
        let joined = a.callback_urls.join("\n");
        ctx.scenario(
            "channel.update.callback",
            OpKind::ChannelUpdate,
            vars(&[("channel_id", &hit.rec.id), ("callback_urls", &joined)]),
        )?;
    }
    if let Some((oa_name, oa_id)) = &linked {
        let out = ctx.scenario(
            "channel.update.linked_oa",
            OpKind::ChannelUpdate,
            vars(&[("channel_id", &hit.rec.id), ("oa_id", oa_id)]),
        )?;
        if out.str("linked_oa").as_deref() != Some(oa_id.as_str()) {
            return Err(LoaError::ui_changed(format!(
                "公式アカウント {oa_name}（{oa_id}）をリンクできませんでした"
            ))
            .with_hint("リンクできるのは、同じプロバイダーでMessaging APIを有効にした公式アカウントだけです"));
        }
    }
    if let Some(n) = &a.name
        && n != &hit.name
    {
        let mut res = ctx.resources()?;
        res.business(&ctx.business_id)
            .put_channel(&hit.provider, "", n, hit.rec.clone());
        ctx.save_resources(&res)?;
    }
    Ok(Envelope::new(
        "channel",
        "updated",
        json!({"id": hit.rec.id, "name": a.name.clone().unwrap_or(hit.name), "changes": changes}),
    ))
}

pub fn remove(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<Envelope> {
    let hit = resolve(ctx, key, provider)?;
    if ctx.dry_run {
        return Ok(planned(
            "channel",
            json!({"delete": {"id": hit.rec.id, "name": hit.name, "provider": hit.provider}, "approval_required": true}),
        ));
    }
    let req = ApprovalRequest::operation(OpKind::ChannelDelete, "チャネルを削除します")
        .target("プロバイダー", &hit.provider)
        .target("チャネル名", &hit.name)
        .target("チャネルID", &hit.rec.id)
        .target("種類", hit.rec.ty.as_str())
        .change("チャネル", Some(hit.name.clone()), Some("（削除）".into()))
        .note("削除したチャネルは元に戻せません。LIFFアプリやログイン連携も使えなくなります。");
    ctx.approve(
        OpKind::ChannelDelete,
        &PolicyContext::default(),
        Some(&hit.provider),
        req,
    )?;
    ctx.scenario(
        "channel.delete",
        OpKind::ChannelDelete,
        vars(&[("channel_id", &hit.rec.id), ("name", &hit.name)]),
    )?;
    let mut res = ctx.resources()?;
    res.business(&ctx.business_id)
        .remove_channel(&hit.provider, &hit.key);
    ctx.save_resources(&res)?;
    for key in [KEY_CHANNEL_SECRET, KEY_ACCESS_TOKEN] {
        ctx.secrets
            .delete(&Ctx::secret_ref(&hit, key).store_key())?;
    }
    Ok(Envelope::new(
        "channel",
        "deleted",
        json!({"id": hit.rec.id, "name": hit.name, "provider": hit.provider}),
    ))
}

/// Save the long-lived channel access token of a Messaging API channel in the keychain: read the
/// one the console shows, issue it when there is none, or reissue it (`reissue`, approval
/// required: the old token stops working).
pub fn issue_token(
    ctx: &mut Ctx<'_>,
    key: &str,
    provider: Option<&str>,
    reissue: bool,
) -> Result<Envelope> {
    let hit = resolve(ctx, key, provider)?;
    if matches!(hit.rec.ty, ChannelType::Login | ChannelType::Miniapp) {
        return Err(LoaError::usage(
            "チャネルアクセストークン（長期）を発行できるのは Messaging APIチャネルだけです",
        ));
    }
    if ctx.dry_run {
        return Ok(planned(
            "channel",
            json!({"issue_token": {"id": hit.rec.id, "name": hit.name, "reissue": reissue}, "approval_required": reissue}),
        ));
    }
    let vars = vars(&[("channel_id", &hit.rec.id)]);
    let out = if reissue {
        let req = ApprovalRequest::operation(
            OpKind::ChannelReissueToken,
            "チャネルアクセストークン（長期）を再発行します",
        )
        .target("プロバイダー", &hit.provider)
        .target("チャネル", format!("{} ({})", hit.name, hit.rec.id))
        .change(
            "チャネルアクセストークン（長期）",
            Some("いまのトークン".into()),
            Some("新しいトークン".into()),
        )
        .note("再発行すると、いまのトークンを使っているサーバーやツールからは、このチャネルを使えなくなります。");
        ctx.approve(
            OpKind::ChannelReissueToken,
            &PolicyContext::default(),
            Some(&hit.provider),
            req,
        )?;
        ctx.scenario("channel.reissue_token", OpKind::ChannelReissueToken, vars)?
    } else {
        // 2026-11-24 実画面: 「発行」は効いたのに画面が書き換わらず失敗したことがある。もう一度流せば
        // 発行済みのトークンを読むだけなので（再発行はしない）、それを次の手として出す
        ctx.scenario("channel.issue_token", OpKind::ChannelIssueToken, vars)
            .map_err(|e| {
                e.with_next(format!(
                    "loa channel issue-token {} --provider \"{}\"",
                    hit.rec.id, hit.provider
                ))
            })?
    };
    let token = out.secrets.get(KEY_ACCESS_TOKEN).ok_or_else(|| {
        LoaError::ui_changed("チャネルアクセストークン（長期）を読み取れませんでした")
    })?;
    if reissue
        && let Ok(old) = ctx.channel_access_token(&hit)
        && old.as_str() == token.as_str()
    {
        return Err(LoaError::ui_changed(
            "再発行のあとも前のトークンが表示されています。コンソールで再発行されたか確かめてください",
        ));
    }
    let token_ref = store_secret(ctx, &hit.provider, &hit.rec.id, KEY_ACCESS_TOKEN, token)?;
    Ok(Envelope::new(
        "channel",
        if reissue { "reissued" } else { "got" },
        json!({"id": hit.rec.id, "name": hit.name, "provider": hit.provider, "token_ref": token_ref}),
    )
    .with_next(format!(
        "loa env write --channel {} --provider \"{}\" --map LINE_CHANNEL_ACCESS_TOKEN=channel.access_token",
        hit.rec.id, hit.provider
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messaging_type_is_refused_with_next_step() {
        let e = parse_type("messaging").unwrap_err();
        assert_eq!(e.code, crate::error::Code::Usage);
        assert!(e.next[0].contains("enable-messaging-api"));
    }

    #[test]
    fn type_label_split() {
        assert_eq!(
            split_type("acme-bot Messaging API"),
            ("acme-bot".into(), ChannelType::Messaging)
        );
        assert_eq!(
            split_type("LINEログイン acme"),
            ("acme".into(), ChannelType::Login)
        );
        assert_eq!(split_type("plain"), ("plain".into(), ChannelType::Unknown));
    }

    #[test]
    fn console_card_uses_heading_and_type_badge() {
        // Shape of a channel card on the provider page (2026-11-21).
        let v = serde_json::json!({
            "id": "2012676868",
            "name": "ASSISTAND",
            "heading": true,
            "parts": ["Admin", "ASSISTAND", "LINEミニアプリ", "未認証"],
        });
        assert_eq!(
            card(&v),
            Some(Card {
                id: "2012676868".into(),
                name: "ASSISTAND".into(),
                ty: ChannelType::Miniapp,
            })
        );
        let v = serde_json::json!({
            "id": "1", "name": "デモ：ASSISTAND", "heading": true, "parts": ["Admin", "デモ：ASSISTAND", "Messaging API"],
        });
        assert_eq!(card(&v).unwrap().ty, ChannelType::Messaging);
        // A card without a heading: the link text holds name and type.
        let v = serde_json::json!({
            "id": "2", "name": "acme LINEログイン", "heading": false, "parts": ["acme", "LINEログイン"],
        });
        let c = card(&v).unwrap();
        assert_eq!((c.name.as_str(), c.ty), ("acme", ChannelType::Login));
    }
}
