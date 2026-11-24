//! Webhook URL of a Messaging API channel through the Messaging API.

use super::{Ctx, channel, planned};
use crate::error::{LoaError, Result};
use crate::gate::ApprovalRequest;
use crate::ops::{OpKind, PolicyContext};
use crate::output::Envelope;
use crate::resources::{ChannelHit, ChannelType};
use serde_json::json;

fn messaging_channel(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<ChannelHit> {
    let hit = channel::resolve(ctx, key, provider)?;
    match hit.rec.ty {
        ChannelType::Login | ChannelType::Miniapp => Err(LoaError::usage(
            "Webhookを設定できるのは Messaging APIチャネルだけです",
        )),
        _ => Ok(hit),
    }
}

pub fn get(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>) -> Result<Envelope> {
    let hit = messaging_channel(ctx, key, provider)?;
    let token = ctx.channel_token(&hit)?;
    let info = ctx.api()?.webhook_get(&token)?;
    Ok(Envelope::new(
        "webhook",
        "got",
        json!({"channel": hit.name, "channel_id": hit.rec.id, "endpoint": info.endpoint, "active": info.active}),
    ))
}

pub fn set(ctx: &mut Ctx<'_>, key: &str, provider: Option<&str>, url: &str) -> Result<Envelope> {
    if !url.starts_with("https://") {
        return Err(LoaError::usage("Webhook URLは https:// で始めてください"));
    }
    let hit = messaging_channel(ctx, key, provider)?;
    let token = ctx.channel_token(&hit)?;
    let api = ctx.api()?;
    let current = api.webhook_get(&token)?;
    if current.endpoint == url {
        return Ok(Envelope::new(
            "webhook",
            "unchanged",
            json!({"channel": hit.name, "endpoint": url, "active": current.active}),
        ));
    }
    let policy = PolicyContext {
        existing_value: Some(current.endpoint.clone()),
        new_value: Some(url.to_string()),
    };
    if ctx.dry_run {
        return Ok(planned(
            "webhook",
            json!({"set": {"channel": hit.name, "endpoint": {"before": current.endpoint, "after": url}}, "approval_required": OpKind::WebhookSet.requires_approval(&policy)}),
        ));
    }
    let req = ApprovalRequest::operation(OpKind::WebhookSet, "Webhook URLを変更します")
        .target("チャネル", format!("{} ({})", hit.name, hit.rec.id))
        .change(
            "Webhook URL",
            Some(current.endpoint.clone()),
            Some(url.to_string()),
        )
        .note("変更した時点から、LINEからのイベントは新しいURLに送られます。");
    ctx.approve(OpKind::WebhookSet, &policy, Some(&hit.provider), req)?;
    api.webhook_set(&token, url)?;
    let mut env = Envelope::new(
        "webhook",
        "updated",
        json!({"channel": hit.name, "endpoint": url, "previous": current.endpoint, "active": current.active}),
    )
    .with_next(format!("loa webhook test --channel {}", hit.key));
    if !current.active {
        env = env.with_warning(
            "Webhookの利用がオフになっています。公式アカウントの応答設定で Webhook をオンにしてください（loa oa update <oa> --response.mode webhook）",
        );
    }
    Ok(env)
}

pub fn test(
    ctx: &mut Ctx<'_>,
    key: &str,
    provider: Option<&str>,
    url: Option<&str>,
) -> Result<Envelope> {
    let hit = messaging_channel(ctx, key, provider)?;
    let token = ctx.channel_token(&hit)?;
    let r = ctx.api()?.webhook_test(&token, url)?;
    let mut env = Envelope::new("webhook", "tested", serde_json::to_value(&r)?);
    if !r.success {
        env = env.with_warning(format!(
            "Webhookのテストに失敗しました: {} {}",
            r.reason.clone().unwrap_or_default(),
            r.detail.clone().unwrap_or_default()
        ));
    }
    Ok(env)
}
