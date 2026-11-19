//! LINE MINI App channels: three internal channels (developing / review / published),
//! one LIFF id each (v1.1 change 2). Creation and deletion go through `channel`.

use super::{Ctx, channel, planned, vars};
use crate::error::{LoaError, Result};
use crate::ops::OpKind;
use crate::output::Envelope;
use crate::resources::{ChannelHit, ChannelType};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const STAGES: [&str; 3] = ["developing", "review", "published"];

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct UpdateArgs {
    pub channel: String,
    #[serde(default)]
    pub provider: Option<String>,
    /// developing | review | published
    pub stage: String,
    pub endpoint_url: String,
}

fn miniapp_channel(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<ChannelHit> {
    let hit = channel::resolve(ctx, key, provider)?;
    match hit.rec.ty {
        ChannelType::Login | ChannelType::Messaging => Err(LoaError::usage(format!(
            "{} はLINEミニアプリチャネルではありません",
            hit.name
        ))),
        _ => Ok(hit),
    }
}

pub fn get(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<Envelope> {
    let hit = miniapp_channel(ctx, key, provider)?;
    let out = ctx.scenario(
        "miniapp.get",
        OpKind::MiniappGet,
        vars(&[("channel_id", &hit.rec.id)]),
    )?;
    let mut stages = serde_json::Map::new();
    let mut res = ctx.resources()?;
    let rec = res
        .business(&ctx.business_id)
        .channel_mut(&hit.provider, &hit.name);
    let mut ids = std::collections::BTreeMap::new();
    for s in STAGES {
        let liff = out.str(&format!("{s}_liff_id")).filter(|v| !v.is_empty());
        let url = out.str(&format!("{s}_endpoint")).filter(|v| !v.is_empty());
        if let Some(l) = &liff {
            ids.insert(s.to_string(), l.clone());
        }
        stages.insert(s.into(), json!({"liff_id": liff, "endpoint_url": url}));
    }
    if let Some(c) = rec {
        c.miniapp = ids;
        c.ty = ChannelType::Miniapp;
    }
    ctx.save_resources(&res)?;
    Ok(Envelope::new(
        "miniapp",
        "got",
        json!({"channel": hit.name, "channel_id": hit.rec.id, "stages": stages}),
    ))
}

pub fn update(ctx: &mut Ctx<'_>, a: &UpdateArgs) -> Result<Envelope> {
    if !STAGES.contains(&a.stage.as_str()) {
        return Err(LoaError::usage(
            "--stage は developing / review / published のいずれかです",
        ));
    }
    if !a.endpoint_url.starts_with("https://") {
        return Err(LoaError::usage(
            "エンドポイントURLは https:// で始めてください",
        ));
    }
    let hit = miniapp_channel(ctx, &a.channel, a.provider.as_deref())?;
    if ctx.dry_run {
        return Ok(planned(
            "miniapp",
            json!({"update": {"channel": hit.name, "stage": a.stage, "endpoint_url": a.endpoint_url}}),
        ));
    }
    let scenario = format!("miniapp.update.{}", a.stage);
    ctx.scenario(
        &scenario,
        OpKind::MiniappUpdate,
        vars(&[
            ("channel_id", &hit.rec.id),
            ("endpoint_url", &a.endpoint_url),
        ]),
    )?;
    Ok(Envelope::new(
        "miniapp",
        "updated",
        json!({"channel": hit.name, "stage": a.stage, "endpoint_url": a.endpoint_url}),
    ))
}
