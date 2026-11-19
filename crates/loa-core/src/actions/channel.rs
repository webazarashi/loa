//! channel ls / get / create / ensure / update / rm

use super::{Ctx, planned, provider, vars};
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

/// (provider name, provider id, [(channel name, channel id)])
pub type Listing = (String, String, Vec<(String, String)>);

/// Channels of a provider from the console; refreshes the local map.
pub fn fetch(ctx: &mut Ctx<'_>, provider_key: &str) -> Result<Listing> {
    let (pname, pid) = provider::resolve_id(ctx, provider_key)?;
    let out = ctx.scenario(
        "channel.list",
        OpKind::ChannelList,
        vars(&[("provider_id", &pid)]),
    )?;
    let items: Vec<(String, String)> = out
        .values
        .get("channels")
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
    let prov = biz.providers.entry(pname.clone()).or_default();
    prov.id = pid.clone();
    for (id, raw_name) in &items {
        let (name, ty) = split_type(raw_name);
        let existing = prov
            .channels
            .iter()
            .find(|(_, c)| &c.id == id)
            .map(|(n, c)| (n.clone(), c.clone()));
        let mut rec = existing
            .as_ref()
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        rec.id = id.clone();
        if ty != ChannelType::Unknown {
            rec.ty = ty;
        }
        if let Some((old, _)) = existing
            && old != name
        {
            prov.channels.remove(&old);
        }
        prov.channels.insert(name, rec);
    }
    // Channels deleted in the console disappear from the map. An empty listing is not trusted
    // (a broken list scenario would otherwise make `ensure` create duplicates).
    let stale_kept = if items.is_empty() {
        !prov.channels.is_empty()
    } else {
        let live: std::collections::HashSet<&str> =
            items.iter().map(|(id, _)| id.as_str()).collect();
        prov.channels
            .retain(|_, c| c.id.is_empty() || live.contains(c.id.as_str()));
        false
    };
    ctx.save_resources(&res)?;
    if stale_kept {
        ctx.warnings.push(format!(
            "コンソールでプロバイダー {pname} のチャネルを読み取れませんでした。手元の対応表のまま進めます"
        ));
    }
    Ok((pname, pid, items))
}

/// Channel cards show the type next to the name; strip it if present.
fn split_type(raw: &str) -> (String, ChannelType) {
    const TYPES: [(&str, ChannelType); 6] = [
        ("Messaging API", ChannelType::Messaging),
        ("LINEログイン", ChannelType::Login),
        ("LINE Login", ChannelType::Login),
        ("LINEミニアプリ", ChannelType::Miniapp),
        ("LINE MINI App", ChannelType::Miniapp),
        ("ミニアプリ", ChannelType::Miniapp),
    ];
    let s = raw.trim();
    for (label, ty) in TYPES {
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
    let (pname, _, _) = fetch(ctx, provider_key)?;
    let res = ctx.resources()?;
    let chans: Vec<Value> = res
        .business_ref(&ctx.business_id)
        .and_then(|b| b.providers.get(&pname))
        .map(|p| {
            p.channels
                .iter()
                .map(|(n, c)| json!({"id": c.id, "name": n, "type": c.ty.as_str()}))
                .collect()
        })
        .unwrap_or_default();
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

fn store_secret(
    ctx: &Ctx<'_>,
    hit_provider: &str,
    name: &str,
    key: &str,
    value: &str,
) -> Result<String> {
    let r = SecretRef::new(hit_provider, name, key);
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
    let mut data = json!({
        "id": hit.rec.id,
        "name": hit.name,
        "provider": hit.provider,
        "type": hit.rec.ty.as_str(),
        "description": out.str("description"),
    });
    if let Some(s) = out.secrets.get("channel_secret") {
        data["secret_ref"] = json!(store_secret(
            ctx,
            &hit.provider,
            &hit.name,
            "channel_secret",
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
    let (pname, pid, _) = fetch(ctx, &a.provider)?;
    if let Ok(hit) = ctx.find_channel(&a.name, Some(&pname)) {
        if !ensure {
            return Err(LoaError::usage(format!(
                "チャネル {} は既にあります（ID {}）",
                a.name, hit.rec.id
            ))
            .with_next(format!("loa channel get {} --provider {pname}", a.name)));
        }
        // ensure: apply differences that can be compared locally (description / callback urls)
        let wants_update =
            a.description.is_some() || !a.callback_urls.is_empty() || a.email.is_some();
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
    res.business(&ctx.business_id).upsert_channel(
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
    if let Some(s) = out.secrets.get("channel_secret") {
        data["secret_ref"] = json!(store_secret(ctx, &pname, &a.name, "channel_secret", s)?);
    }
    let mut env = Envelope::new("channel", "created", data);
    if ty == ChannelType::Login {
        if !a.callback_urls.is_empty() {
            let u = update(
                ctx,
                &UpdateArgs {
                    channel: a.name.clone(),
                    provider: Some(pname.clone()),
                    callback_urls: a.callback_urls.clone(),
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
    if changes.is_empty() {
        return Err(LoaError::usage(
            "変更する項目を指定してください（--name / --description / --email / --callback-url）",
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
    if let Some(n) = &a.name
        && n != &hit.name
    {
        let mut res = ctx.resources()?;
        let biz = res.business(&ctx.business_id);
        if let Some(p) = biz.providers.get_mut(&hit.provider)
            && let Some(rec) = p.channels.remove(&hit.name)
        {
            p.channels.insert(n.clone(), rec);
        }
        ctx.save_resources(&res)?;
        // move the stored secret to the new name
        let key = "channel_secret";
        let old = SecretRef::new(&hit.provider, &hit.name, key);
        if let Some(v) = ctx.secrets.get(&old.store_key())? {
            ctx.secrets
                .set(&SecretRef::new(&hit.provider, n, key).store_key(), &v)?;
            ctx.secrets.delete(&old.store_key())?;
        }
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
        .remove_channel(&hit.provider, &hit.name);
    ctx.save_resources(&res)?;
    ctx.secrets
        .delete(&SecretRef::new(&hit.provider, &hit.name, "channel_secret").store_key())?;
    Ok(Envelope::new(
        "channel",
        "deleted",
        json!({"id": hit.rec.id, "name": hit.name, "provider": hit.provider}),
    ))
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
}
