//! `loa selftest`: create → get → update → delete on a dedicated test provider and list
//! scenarios that broke (spec 7-5). Deletes are covered by one bulk approval that only
//! applies to the selftest provider.

use super::{Ctx, channel, liff, provider};
use crate::error::{Code, LoaError, Result};
use crate::gate::{ApprovalKind, ApprovalRequest};
use crate::output::Envelope;
use serde_json::{Value, json};

pub const PREFIX: &str = "loa-selftest";

pub fn run(ctx: &mut Ctx<'_>, provider_name: &str) -> Result<Envelope> {
    if !provider_name.starts_with(PREFIX) {
        return Err(LoaError::usage(format!(
            "selftest のプロバイダー名は {PREFIX} で始めてください（本番のプロバイダーを誤って操作しないため）"
        )));
    }
    if ctx.dry_run {
        return Err(LoaError::usage("selftest は --dry-run に対応していません"));
    }
    let suffix = &crate::util::random_hex(2);
    let ch = format!("loa-st-{suffix}");
    let req = ApprovalRequest {
        kind: ApprovalKind::Bulk,
        title: format!(
            "selftest: テスト用プロバイダー {provider_name} で作成・更新・削除を行います"
        ),
        operation: "selftest".into(),
        target: vec![
            ("プロバイダー".into(), provider_name.into()),
            ("作成するチャネル".into(), ch.clone()),
        ],
        changes: vec![],
        link: None,
        note: Some("この承認は selftest 中の、このプロバイダー配下の削除にだけ使われます。".into()),
    };
    if !ctx.gate.approve(&req)? {
        return Err(LoaError::new(
            Code::ApprovalDenied,
            "selftest が承認されませんでした",
        ));
    }
    ctx.set_bulk_approved(provider_name);

    let mut results: Vec<Value> = Vec::new();
    let mut record = |name: &str, scenarios: &str, r: Result<Envelope>| -> bool {
        let ok = r.is_ok();
        let mut row = json!({"step": name, "scenarios": scenarios, "ok": ok});
        if let Err(e) = r {
            row["code"] = json!(e.code);
            row["message"] = json!(crate::redact::redact(&e.message));
            if let Some(rp) = e.report {
                row["report"] = json!(rp);
            }
        }
        results.push(row);
        ok
    };

    let ok_provider = record(
        "provider ensure",
        "provider.list, provider.create",
        provider::create(ctx, provider_name, true),
    );
    if ok_provider {
        let args = channel::CreateArgs {
            provider: provider_name.into(),
            ty: "login".into(),
            name: ch.clone(),
            description: Some("loa selftest".into()),
            email: None,
            callback_urls: vec![],
        };
        let created = record(
            "channel create",
            "channel.list, channel.create.login",
            channel::create(ctx, &args, false),
        );
        if created {
            record(
                "channel get",
                "channel.get",
                channel::get(ctx, &ch, Some(provider_name)),
            );
            record(
                "channel update",
                "channel.update.basic",
                channel::update(
                    ctx,
                    &channel::UpdateArgs {
                        channel: ch.clone(),
                        provider: Some(provider_name.into()),
                        description: Some("loa selftest (updated)".into()),
                        ..Default::default()
                    },
                ),
            );
            record(
                "channel callback",
                "channel.update.callback",
                channel::update(
                    ctx,
                    &channel::UpdateArgs {
                        channel: ch.clone(),
                        provider: Some(provider_name.into()),
                        callback_urls: vec!["https://example.com/callback".into()],
                        ..Default::default()
                    },
                ),
            );
            let added = liff::add(
                ctx,
                &liff::AddArgs {
                    channel: ch.clone(),
                    provider: Some(provider_name.into()),
                    name: "selftest".into(),
                    url: "https://example.com/".into(),
                    size: Some("full".into()),
                    scope: None,
                    bot_prompt: None,
                },
                false,
            );
            let liff_id = added.as_ref().ok().and_then(|e| {
                e.data
                    .get("liff_id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
            record("liff add", "(LIFF API)", added);
            if let Some(id) = liff_id {
                record(
                    "liff update",
                    "(LIFF API)",
                    liff::update(
                        ctx,
                        &liff::UpdateArgs {
                            channel: ch.clone(),
                            provider: Some(provider_name.into()),
                            liff_id: id.clone(),
                            url: Some("https://example.com/updated".into()),
                            ..Default::default()
                        },
                    ),
                );
                record(
                    "liff rm",
                    "(LIFF API)",
                    liff::remove(ctx, &ch, Some(provider_name), &id),
                );
            }
            record(
                "channel rm",
                "channel.delete",
                channel::remove(ctx, &ch, Some(provider_name)),
            );
        }
    }
    let failed: Vec<&Value> = results.iter().filter(|r| r["ok"] == json!(false)).collect();
    let data = json!({"provider": provider_name, "channel": ch, "results": results});
    if failed.is_empty() {
        Ok(Envelope::new("selftest", "passed", data))
    } else {
        let broken: Vec<String> = failed
            .iter()
            .map(|r| r["scenarios"].as_str().unwrap_or("").to_string())
            .collect();
        // A person said no (or did not answer): the scenarios are not known to be broken.
        if let Some(denied) = failed.iter().find(|r| r["code"] == "APPROVAL_DENIED") {
            return Err(LoaError::new(
                Code::ApprovalDenied,
                format!(
                    "selftest を中止しました: {}",
                    denied["message"].as_str().unwrap_or("承認されませんでした")
                ),
            )
            .with_detail(data));
        }
        Err(LoaError::new(
            Code::UiChanged,
            format!(
                "selftest で {} 件失敗しました: {}",
                failed.len(),
                broken.join(" / ")
            ),
        )
        .with_detail(data)
        .with_next("loa scenarios update"))
    }
}
