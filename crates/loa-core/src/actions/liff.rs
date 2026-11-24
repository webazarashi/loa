//! LIFF apps through the LIFF Server API (LINE Login channels only).

use super::{Ctx, channel, planned};
use crate::error::{LoaError, Result};
use crate::gate::ApprovalRequest;
use crate::line_api::{LiffApp, LiffInput};
use crate::ops::{OpKind, PolicyContext};
use crate::output::Envelope;
use crate::resources::{ChannelHit, ChannelType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct AddArgs {
    pub channel: String,
    #[serde(default)]
    pub provider: Option<String>,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub scope: Option<Vec<String>>,
    #[serde(default)]
    pub bot_prompt: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UpdateArgs {
    pub channel: String,
    #[serde(default)]
    pub provider: Option<String>,
    pub liff_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub bot_prompt: Option<String>,
}

/// Channel that owns a LIFF id, from the local map (for `loa liff rm <liffId>` without --channel).
pub fn channel_for_liff(ctx: &Ctx<'_>, liff_id: &str) -> Result<String> {
    let res = ctx.resources()?;
    let mut hits = Vec::new();
    if let Some(b) = res.business_ref(&ctx.business_id) {
        for p in b.providers.values() {
            for (n, c) in &p.channels {
                if c.liff.values().any(|v| v == liff_id) {
                    hits.push(n.clone());
                }
            }
        }
    }
    match hits.len() {
        1 => Ok(hits.remove(0)),
        _ => Err(LoaError::usage(format!(
            "LIFF ID {liff_id} のチャネルが分かりません。--channel を指定してください"
        ))),
    }
}

fn login_channel(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<ChannelHit> {
    let hit = channel::resolve(ctx, key, provider)?;
    match hit.rec.ty {
        ChannelType::Miniapp => Err(LoaError::usage(
            "LINEミニアプリチャネルのLIFF IDは内部チャネルごとに固定です。loa miniapp get / update を使ってください",
        )
        .with_next(format!("loa miniapp get {}", hit.key))),
        ChannelType::Messaging => Err(LoaError::usage(
            "Messaging APIチャネルにはLIFFアプリを追加できません。LINEログインチャネルを指定してください",
        )),
        _ => Ok(hit),
    }
}

fn app_json(a: &LiffApp) -> Value {
    json!({
        "liff_id": a.liff_id,
        "name": a.description,
        "url": a.view.url,
        "size": a.view.ty,
        "liff_url": format!("https://liff.line.me/{}", a.liff_id),
    })
}

fn remember(ctx: &Ctx<'_>, hit: &ChannelHit, apps: &[LiffApp]) -> Result<()> {
    let mut res = ctx.resources()?;
    if let Some(c) = res
        .business(&ctx.business_id)
        .channel_mut(&hit.provider, &hit.key)
    {
        c.liff = apps
            .iter()
            .filter_map(|a| Some((a.description.clone()?, a.liff_id.clone())))
            .collect();
        if c.ty == ChannelType::Unknown {
            c.ty = ChannelType::Login;
        }
    }
    ctx.save_resources(&res)
}

pub fn fetch(ctx: &mut Ctx<'_>, hit: &ChannelHit) -> Result<Vec<LiffApp>> {
    let token = ctx.channel_token(hit)?;
    let apps = ctx.api()?.liff_list(&token)?;
    remember(ctx, hit, &apps)?;
    Ok(apps)
}

pub fn list(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<Envelope> {
    let hit = login_channel(ctx, key, provider)?;
    let apps = fetch(ctx, &hit)?;
    Ok(Envelope::new(
        "liff",
        "listed",
        Value::Array(apps.iter().map(app_json).collect()),
    ))
}

pub fn get(
    ctx: &mut Ctx<'_>,
    key: &str,
    provider: Option<&str>,
    liff_id: &str,
) -> Result<Envelope> {
    let key = if key.is_empty() {
        channel_for_liff(ctx, liff_id)?
    } else {
        key.to_string()
    };
    let hit = login_channel(ctx, &key, provider)?;
    let apps = fetch(ctx, &hit)?;
    let a = apps
        .iter()
        .find(|a| a.liff_id == liff_id || a.description.as_deref() == Some(liff_id))
        .ok_or_else(|| LoaError::usage(format!("LIFFアプリ {liff_id} が見つかりません")))?;
    Ok(Envelope::new("liff", "got", app_json(a)))
}

pub fn add(ctx: &mut Ctx<'_>, a: &AddArgs, ensure: bool) -> Result<Envelope> {
    let input = LiffInput {
        name: Some(a.name.clone()),
        url: Some(a.url.clone()),
        size: Some(a.size.clone().unwrap_or_else(|| "full".into())),
        scope: a.scope.clone(),
        bot_prompt: a.bot_prompt.clone(),
    };
    input.validate(true)?;
    let hit = login_channel(ctx, &a.channel, a.provider.as_deref())?;
    let apps = fetch(ctx, &hit)?;
    if let Some(existing) = apps
        .iter()
        .find(|x| x.description.as_deref() == Some(a.name.as_str()))
    {
        if !ensure {
            return Err(LoaError::usage(format!(
                "LIFFアプリ {} は既にあります（{}）",
                a.name, existing.liff_id
            ))
            .with_next(format!(
                "loa liff ensure --channel {} --name {} --url {}",
                hit.key, a.name, a.url
            )));
        }
        let want_size = input.size.clone().unwrap_or_default();
        let differs = existing.view.url != a.url || existing.view.ty != want_size;
        if !differs {
            return Ok(Envelope::new("liff", "unchanged", app_json(existing)));
        }
        if ctx.dry_run {
            return Ok(planned(
                "liff",
                json!({"update": {"liff_id": existing.liff_id, "url": {"before": existing.view.url, "after": a.url}, "size": {"before": existing.view.ty, "after": want_size}}}),
            ));
        }
        let token = ctx.channel_token(&hit)?;
        ctx.api()?.liff_update(
            &token,
            &existing.liff_id,
            &LiffInput {
                url: Some(a.url.clone()),
                size: Some(want_size.clone()),
                ..Default::default()
            },
        )?;
        let apps = fetch(ctx, &hit)?;
        let updated = apps
            .iter()
            .find(|x| x.liff_id == existing.liff_id)
            .unwrap_or(existing);
        return Ok(Envelope::new("liff", "updated", app_json(updated)));
    }
    if apps.len() >= 30 {
        return Err(LoaError::usage(
            "1つのチャネルに追加できるLIFFアプリは30個までです",
        ));
    }
    if ctx.dry_run {
        return Ok(planned(
            "liff",
            json!({"add": {"channel": hit.name, "name": a.name, "url": a.url}}),
        ));
    }
    let token = ctx.channel_token(&hit)?;
    let id = ctx.api()?.liff_add(&token, &input)?;
    fetch(ctx, &hit)?;
    Ok(Envelope::new(
        "liff",
        "created",
        json!({"liff_id": id, "name": a.name, "url": a.url, "size": input.size, "liff_url": format!("https://liff.line.me/{id}")}),
    )
    .with_next(format!(
        "loa env write --channel {} --out .env.local --map LIFF_ID=liff.{}.id",
        hit.key, a.name
    )))
}

pub fn update(ctx: &mut Ctx<'_>, a: &UpdateArgs) -> Result<Envelope> {
    let input = LiffInput {
        name: a.name.clone(),
        url: a.url.clone(),
        size: a.size.clone(),
        scope: None,
        bot_prompt: a.bot_prompt.clone(),
    };
    input.validate(false)?;
    if input == LiffInput::default() {
        return Err(LoaError::usage(
            "変更する項目を指定してください（--name / --url / --size / --bot-prompt）",
        ));
    }
    let key = if a.channel.is_empty() {
        channel_for_liff(ctx, &a.liff_id)?
    } else {
        a.channel.clone()
    };
    let hit = login_channel(ctx, &key, a.provider.as_deref())?;
    let apps = fetch(ctx, &hit)?;
    let current = apps
        .iter()
        .find(|x| x.liff_id == a.liff_id)
        .ok_or_else(|| LoaError::usage(format!("LIFFアプリ {} が見つかりません", a.liff_id)))?;
    if ctx.dry_run {
        return Ok(planned(
            "liff",
            json!({"update": {"liff_id": a.liff_id, "changes": input}}),
        ));
    }
    let token = ctx.channel_token(&hit)?;
    ctx.api()?.liff_update(&token, &current.liff_id, &input)?;
    let apps = fetch(ctx, &hit)?;
    let updated = apps.iter().find(|x| x.liff_id == a.liff_id).cloned();
    Ok(Envelope::new(
        "liff",
        "updated",
        updated
            .as_ref()
            .map(app_json)
            .unwrap_or(json!({"liff_id": a.liff_id})),
    ))
}

pub fn remove(
    ctx: &mut Ctx<'_>,
    key: &str,
    provider: Option<&str>,
    liff_id: &str,
) -> Result<Envelope> {
    let key = if key.is_empty() {
        channel_for_liff(ctx, liff_id)?
    } else {
        key.to_string()
    };
    let hit = login_channel(ctx, &key, provider)?;
    let apps = fetch(ctx, &hit)?;
    let app = apps
        .iter()
        .find(|x| x.liff_id == liff_id)
        .ok_or_else(|| LoaError::usage(format!("LIFFアプリ {liff_id} が見つかりません")))?
        .clone();
    if ctx.dry_run {
        return Ok(planned(
            "liff",
            json!({"delete": app_json(&app), "approval_required": true}),
        ));
    }
    let req = ApprovalRequest::operation(OpKind::LiffDelete, "LIFFアプリを削除します")
        .target("チャネル", format!("{} ({})", hit.name, hit.rec.id))
        .target("LIFF ID", &app.liff_id)
        .target("名前", app.description.clone().unwrap_or_default())
        .target("エンドポイントURL", &app.view.url)
        .change(
            "LIFFアプリ",
            Some(app.liff_id.clone()),
            Some("（削除）".into()),
        )
        .note("このLIFF IDを使っているアプリは動かなくなります。");
    ctx.approve(
        OpKind::LiffDelete,
        &PolicyContext::default(),
        Some(&hit.provider),
        req,
    )?;
    let token = ctx.channel_token(&hit)?;
    ctx.api()?.liff_delete(&token, &app.liff_id)?;
    fetch(ctx, &hit)?;
    Ok(Envelope::new("liff", "deleted", app_json(&app)))
}
