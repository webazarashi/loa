//! login / whoami / logout

use super::{Ctx, vars};
use crate::config::validate_business_id;
use crate::domains::is_login_url;
use crate::error::Result;
use crate::ops::OpKind;
use crate::output::Envelope;
use serde_json::json;

pub fn login(ctx: &mut Ctx<'_>) -> Result<Envelope> {
    validate_business_id(&ctx.business_id)?;
    if ctx.dry_run {
        return Ok(super::planned(
            "session",
            json!({"login": {"business_id": ctx.business_id}}),
        ));
    }
    let out = ctx.scenario("session.login", OpKind::SessionLogin, vars(&[]))?;
    let mut cfg = crate::config::Config::load(&ctx.paths)?;
    cfg.remember_business(&ctx.business_id);
    cfg.save(&ctx.paths)?;
    Ok(Envelope::new(
        "session",
        "logged_in",
        json!({
            "business_id": ctx.business_id,
            "logged_in": true,
            "account": out.str("account"),
        }),
    )
    .with_next("loa provider ls"))
}

pub fn whoami(ctx: &mut Ctx<'_>) -> Result<Envelope> {
    validate_business_id(&ctx.business_id)?;
    let out = ctx.scenario("session.whoami", OpKind::SessionWhoami, vars(&[]))?;
    let url = out.str("url").unwrap_or_default();
    let logged_in = !url.is_empty() && !is_login_url(&url);
    let mut env = Envelope::new(
        "session",
        "got",
        json!({
            "business_id": ctx.business_id,
            "logged_in": logged_in,
            "account": if logged_in { out.str("account") } else { None },
        }),
    );
    if !logged_in {
        env = env.with_next(format!("loa login --business-id {}", ctx.business_id));
    }
    Ok(env)
}

pub fn logout(ctx: &mut Ctx<'_>) -> Result<Envelope> {
    validate_business_id(&ctx.business_id)?;
    if ctx.dry_run {
        return Ok(super::planned(
            "session",
            json!({"logout": {"business_id": ctx.business_id}}),
        ));
    }
    let b = ctx.browser()?;
    b.forget_state().map_err(crate::browser::to_loa_error)?;
    Ok(Envelope::new(
        "session",
        "logged_out",
        json!({ "business_id": ctx.business_id, "logged_in": false }),
    ))
}
