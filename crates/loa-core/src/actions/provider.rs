//! provider ls / get / create / ensure / update (no delete: spec 2).

use super::{Ctx, planned, vars};
use crate::error::{LoaError, Result};
use crate::ops::OpKind;
use crate::output::Envelope;
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderItem {
    pub id: String,
    pub name: String,
}

pub fn validate_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(LoaError::usage("プロバイダー名が空です"));
    }
    if name.to_ascii_uppercase().contains("LINE") {
        return Err(LoaError::usage("プロバイダー名に「LINE」は含められません"));
    }
    Ok(())
}

/// Read providers from the console and refresh the local map.
pub fn fetch(ctx: &mut Ctx<'_>) -> Result<Vec<ProviderItem>> {
    let out = ctx.scenario("provider.list", OpKind::ProviderList, vars(&[]))?;
    let items: Vec<ProviderItem> = out
        .values
        .get("providers")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| {
                    Some(ProviderItem {
                        id: v.get("id")?.as_str()?.to_string(),
                        name: v.get("name")?.as_str()?.trim().to_string(),
                    })
                })
                .filter(|p| !p.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut res = ctx.resources()?;
    let biz = res.business(&ctx.business_id);
    for p in &items {
        // rename detection: same id under another name
        let old = biz
            .providers
            .iter()
            .find(|(n, r)| r.id == p.id && **n != p.name)
            .map(|(n, _)| n.clone());
        if let Some(old) = old
            && let Some(rec) = biz.providers.remove(&old)
        {
            biz.providers.insert(p.name.clone(), rec);
        }
        biz.providers.entry(p.name.clone()).or_default().id = p.id.clone();
    }
    ctx.save_resources(&res)?;
    Ok(items)
}

fn to_json(items: &[ProviderItem]) -> Value {
    Value::Array(
        items
            .iter()
            .map(|p| json!({"id": p.id, "name": p.name}))
            .collect(),
    )
}

pub fn list(ctx: &mut Ctx<'_>) -> Result<Envelope> {
    let items = fetch(ctx)?;
    Ok(Envelope::new("provider", "listed", to_json(&items)))
}

pub fn find(ctx: &mut Ctx<'_>, key: &str) -> Result<Option<ProviderItem>> {
    Ok(fetch(ctx)?
        .into_iter()
        .find(|p| p.name == key || p.id == key))
}

/// Provider id by name/id from the local map, refreshing from the console when unknown.
pub fn resolve_id(ctx: &mut Ctx<'_>, key: &str) -> Result<(String, String)> {
    let res = ctx.resources()?;
    if let Some(b) = res.business_ref(&ctx.business_id)
        && let Some((name, p)) = b.provider_by_name_or_id(key)
        && !p.id.is_empty()
    {
        return Ok((name, p.id.clone()));
    }
    match find(ctx, key)? {
        Some(p) => Ok((p.name, p.id)),
        None => Err(
            LoaError::usage(format!("プロバイダー {key} が見つかりません"))
                .with_next(format!("loa provider ensure --name {key}")),
        ),
    }
}

pub fn get(ctx: &mut Ctx<'_>, key: &str) -> Result<Envelope> {
    match find(ctx, key)? {
        Some(p) => Ok(
            Envelope::new("provider", "got", json!({"id": p.id, "name": p.name}))
                .with_next(format!("loa channel ls --provider {}", p.name)),
        ),
        None => Err(LoaError::usage(format!(
            "プロバイダー {key} が見つかりません"
        ))),
    }
}

pub fn create(ctx: &mut Ctx<'_>, name: &str, ensure: bool) -> Result<Envelope> {
    validate_name(name)?;
    if let Some(p) = find(ctx, name)? {
        if ensure {
            return Ok(Envelope::new(
                "provider",
                "unchanged",
                json!({"id": p.id, "name": p.name}),
            ));
        }
        return Err(
            LoaError::usage(format!("プロバイダー {name} は既にあります（ID {}）", p.id))
                .with_next(format!("loa provider ensure --name {name}")),
        );
    }
    if ctx.dry_run {
        return Ok(planned("provider", json!({"create": {"name": name}})));
    }
    let out = ctx.scenario(
        "provider.create",
        OpKind::ProviderCreate,
        vars(&[("name", name)]),
    )?;
    let id = out.str("provider_id").unwrap_or_default();
    let mut res = ctx.resources()?;
    res.business(&ctx.business_id)
        .providers
        .entry(name.to_string())
        .or_default()
        .id = id.clone();
    ctx.save_resources(&res)?;
    Ok(
        Envelope::new("provider", "created", json!({"id": id, "name": name})).with_next(format!(
            "loa channel ensure --provider {name} --type login --name <channel>"
        )),
    )
}

pub fn update(ctx: &mut Ctx<'_>, key: &str, new_name: &str) -> Result<Envelope> {
    validate_name(new_name)?;
    let (name, id) = resolve_id(ctx, key)?;
    if name == new_name {
        return Ok(Envelope::new(
            "provider",
            "unchanged",
            json!({"id": id, "name": name}),
        ));
    }
    if ctx.dry_run {
        return Ok(planned(
            "provider",
            json!({"update": {"id": id, "name": {"before": name, "after": new_name}}}),
        ));
    }
    ctx.scenario(
        "provider.update",
        OpKind::ProviderUpdate,
        vars(&[("provider_id", &id), ("name", new_name)]),
    )?;
    let mut res = ctx.resources()?;
    let biz = res.business(&ctx.business_id);
    if let Some(rec) = biz.providers.remove(&name) {
        biz.providers.insert(new_name.to_string(), rec);
    }
    ctx.save_resources(&res)?;
    Ok(Envelope::new(
        "provider",
        "updated",
        json!({"id": id, "name": new_name, "previous_name": name}),
    ))
}
