//! `loa` — entry point. Commands that may need a browser or a person run as tasks in a
//! detached worker; local commands (setup, env, exec, schema, scenarios, terms) run here.

// See loa-core: errors end a command, so the size of `LoaError` does not matter.
#![allow(clippy::result_large_err)]

mod cli;
mod schema;

use clap::Parser;
use cli::*;
use loa_core::actions::{self, Command, Ctx, Request, channel, liff, miniapp, oa};
use loa_core::config::Config;
use loa_core::error::{Code, LoaError, Result};
use loa_core::gate::NoHumanGate;
use loa_core::output::{self, Envelope, Outcome};
use loa_core::paths::Paths;
use loa_core::task::{self, FollowEvent, FollowMode};
use serde_json::json;
use std::io::Read;
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let json_flag = args.iter().any(|a| a == "--json");
    let cli = match Cli::try_parse_from(&args) {
        Ok(c) => c,
        Err(e) => {
            use clap::error::ErrorKind;
            if matches!(e.kind(), ErrorKind::DisplayHelp | ErrorKind::DisplayVersion) {
                let _ = e.print();
                std::process::exit(0);
            }
            if json_flag {
                let err = LoaError::usage(
                    e.to_string()
                        .lines()
                        .next()
                        .unwrap_or("引数が不正です")
                        .to_string(),
                )
                .with_next("loa schema <command>");
                println!("{}", output::error_json(&err));
            } else {
                let _ = e.print();
            }
            std::process::exit(Code::Usage.exit_code());
        }
    };
    let json = cli.json;
    let code = match run(cli) {
        Ok(Some(outcome)) => print_outcome(&outcome, json),
        Ok(None) => 0,
        Err(e) => print_outcome(&Err(e), json),
    };
    std::process::exit(code);
}

fn print_outcome(o: &Outcome, json: bool) -> i32 {
    match o {
        Ok(env) => {
            if json {
                println!("{}", output::envelope_json(env));
            } else {
                print!("{}", output::render_human(env));
            }
            0
        }
        Err(e) => {
            if json {
                println!("{}", output::error_json(e));
            } else {
                eprint!("{}", output::render_human_error(e));
            }
            e.exit_code()
        }
    }
}

fn paths() -> Result<Paths> {
    let p = Paths::discover()?;
    p.ensure()?;
    Ok(p)
}

/// Returns None when the command already wrote its own output (exec, __ui, __worker).
fn run(cli: Cli) -> Result<Option<Outcome>> {
    let json = cli.json;
    let dry_run = cli.dry_run;
    let bid = cli.business_id.clone();
    let task_cmd: Option<Command> = match cli.cmd {
        Cmd::Worker { id } => {
            worker(&id);
            return Ok(None);
        }
        Cmd::Ui { kind } => {
            let mut input = String::new();
            let _ = std::io::stdin().read_to_string(&mut input);
            let code = loa_ui::run(&kind, &input);
            std::process::exit(code);
        }
        Cmd::Setup => return Ok(Some(setup())),
        Cmd::Schema { target } => return Ok(Some(schema::run(&target))),
        Cmd::Terms(TermsCmd::Ls) => return Ok(Some(terms_ls())),
        Cmd::Scenarios(s) => return Ok(Some(scenarios(s))),
        Cmd::Wait { task: id, timeout } => {
            let p = paths()?;
            return Ok(Some(follow(
                &p,
                &id,
                FollowMode::UntilDone,
                Some(Duration::from_secs(timeout)),
                json,
            )));
        }
        Cmd::Env(EnvCmd::Write {
            channel,
            provider,
            out,
            maps,
        }) => {
            return Ok(Some(env_write(
                bid.as_deref(),
                &channel,
                provider.as_deref(),
                &out,
                &maps,
            )));
        }
        Cmd::Exec(a) => {
            let code = exec(bid.as_deref(), &a)?;
            std::process::exit(code);
        }
        Cmd::Login => Some(Command::Login),
        Cmd::Whoami => Some(Command::Whoami),
        Cmd::Logout => Some(Command::Logout),
        Cmd::Oa(c) => Some(match c {
            OaCmd::Ls => Command::OaList,
            OaCmd::Get { oa } => Command::OaGet { oa },
            OaCmd::Create {
                name,
                category,
                subcategory,
                company,
            } => Command::OaCreate(oa::CreateArgs {
                name,
                category,
                subcategory,
                company,
            }),
            OaCmd::Update(u) => Command::OaUpdate(oa::UpdateArgs {
                oa: u.oa,
                profile_name: u.profile_name,
                profile_status_message: u.profile_status_message,
                profile_icon: u.profile_icon,
                profile_background: u.profile_background,
                settings_join_groups: u.settings_join_groups,
                members_add: u.members_add,
                members_role: u.members_role,
                members_remove: u.members_remove,
                response_mode: u.response_mode,
            }),
            OaCmd::EnableMessagingApi { oa, provider } => {
                Command::OaEnableMessagingApi { oa, provider }
            }
        }),
        Cmd::Provider(c) => Some(match c {
            ProviderCmd::Ls => Command::ProviderList,
            ProviderCmd::Get { provider } => Command::ProviderGet { provider },
            ProviderCmd::Create { name } => Command::ProviderCreate { name },
            ProviderCmd::Ensure { name } => Command::ProviderEnsure { name },
            ProviderCmd::Update { provider, name } => Command::ProviderUpdate {
                provider,
                new_name: name,
            },
        }),
        Cmd::Channel(c) => Some(match c {
            ChannelCmd::Ls { provider } => Command::ChannelList { provider },
            ChannelCmd::Get { channel, provider } => Command::ChannelGet { channel, provider },
            ChannelCmd::Create(a) => Command::ChannelCreate(create_args(a)),
            ChannelCmd::Ensure(a) => Command::ChannelEnsure(create_args(a)),
            ChannelCmd::Update {
                channel,
                provider,
                name,
                description,
                email,
                callback_urls,
                linked_oa,
            } => Command::ChannelUpdate(channel::UpdateArgs {
                channel,
                provider,
                name,
                description,
                email,
                callback_urls,
                linked_oa,
            }),
            ChannelCmd::Rm { channel, provider } => Command::ChannelRm { channel, provider },
            ChannelCmd::IssueToken {
                channel,
                provider,
                reissue,
            } => Command::ChannelIssueToken {
                channel,
                provider,
                reissue,
            },
        }),
        Cmd::Liff(c) => Some(match c {
            LiffCmd::Ls { channel, provider } => Command::LiffList { channel, provider },
            LiffCmd::Get {
                liff_id,
                channel,
                provider,
            } => Command::LiffGet {
                channel,
                provider,
                liff_id,
            },
            LiffCmd::Add(a) => Command::LiffAdd(add_args(a)),
            LiffCmd::Ensure(a) => Command::LiffEnsure(add_args(a)),
            LiffCmd::Update {
                liff_id,
                channel,
                provider,
                name,
                url,
                size,
                bot_prompt,
            } => Command::LiffUpdate(liff::UpdateArgs {
                channel,
                provider,
                liff_id,
                name,
                url,
                size,
                bot_prompt,
            }),
            LiffCmd::Rm {
                liff_id,
                channel,
                provider,
            } => Command::LiffRm {
                channel,
                provider,
                liff_id,
            },
        }),
        Cmd::Miniapp(c) => Some(match c {
            MiniappCmd::Get { channel, provider } => Command::MiniappGet { channel, provider },
            MiniappCmd::Update {
                channel,
                provider,
                stage,
                endpoint_url,
            } => Command::MiniappUpdate(miniapp::UpdateArgs {
                channel,
                provider,
                stage,
                endpoint_url,
            }),
        }),
        Cmd::Webhook(c) => Some(match c {
            WebhookCmd::Get { channel, provider } => Command::WebhookGet { channel, provider },
            WebhookCmd::Set {
                channel,
                provider,
                url,
            } => Command::WebhookSet {
                channel,
                provider,
                url,
            },
            WebhookCmd::Test {
                channel,
                provider,
                url,
            } => Command::WebhookTest {
                channel,
                provider,
                url,
            },
        }),
        Cmd::Plan { file } => Some(Command::Plan { file }),
        Cmd::Apply { file, prune } => Some(Command::Apply { file, prune }),
        Cmd::Selftest { provider } => Some(Command::Selftest { provider }),
    };
    let cmd = task_cmd.expect("task command");
    let p = paths()?;
    let cfg = Config::load(&p)?;
    let business_id = cfg.business_id(bid.as_deref());
    loa_core::config::validate_business_id(&business_id)?;
    let req = Request {
        command: cmd,
        business_id,
        dry_run,
        cwd: std::env::current_dir()
            .unwrap_or_default()
            .display()
            .to_string(),
    };
    let id = task::create(&p, &serde_json::to_value(&req)?)?;
    let exe = std::env::current_exe()
        .map_err(|e| LoaError::other(format!("実行ファイルの場所が分かりません: {e}")))?;
    task::spawn_worker(&exe, &p, &id)?;
    let mode = if json {
        FollowMode::ExitOnHuman
    } else {
        FollowMode::UntilDone
    };
    Ok(Some(follow(&p, &id, mode, None, json)))
}

fn create_args(a: ChannelCreateArgs) -> channel::CreateArgs {
    channel::CreateArgs {
        provider: a.provider,
        ty: a.ty,
        name: a.name,
        description: a.description,
        email: a.email,
        callback_urls: a.callback_urls,
        linked_oa: a.linked_oa,
    }
}

fn add_args(a: LiffAddArgs) -> liff::AddArgs {
    liff::AddArgs {
        channel: a.channel,
        provider: a.provider,
        name: a.name,
        url: a.url,
        size: a.size,
        scope: if a.scope.is_empty() {
            None
        } else {
            Some(a.scope)
        },
        bot_prompt: a.bot_prompt,
    }
}

fn follow(p: &Paths, id: &str, mode: FollowMode, timeout: Option<Duration>, json: bool) -> Outcome {
    task::follow(p, id, mode, timeout, |ev| match ev {
        FollowEvent::Progress(m) => {
            if !json {
                eprintln!("  {m}");
            }
        }
        FollowEvent::Waiting(w) => {
            if !json {
                let what = match w.code {
                    Code::LoginRequired => "ログイン待ち",
                    Code::ApprovalRequired => "承認待ち",
                    _ => "操作待ち",
                };
                eprintln!("⏳ {what}: {}（タスク {id}）", w.hint);
            }
        }
    })
}

/// The detached worker: runs one task and writes its result.
fn worker(id: &str) {
    let Ok(p) = Paths::discover() else { return };
    let Ok(true) = task::hold_lock(&task::worker_lock_path(&p, id)) else {
        return;
    };
    // `loa wait` may have given up on this task before we started (WORKER_LOST).
    if task::read_status(&p, id).is_ok_and(|s| s.state.is_terminal()) {
        return;
    }
    let _ = task::update_status(&p, id, |s| {
        s.state = task::TaskState::Running;
        s.pid = Some(std::process::id());
    });
    let outcome: Outcome = std::panic::catch_unwind(|| run_task(&p, id)).unwrap_or_else(|_| {
        Err(LoaError::other(
            "内部エラーが発生しました（worker.log を確認してください）",
        ))
    });
    let _ = task::write_result(&p, id, &outcome);
}

fn run_task(p: &Paths, id: &str) -> Outcome {
    let req: Request = serde_json::from_value(task::read_request(p, id)?)?;
    let cfg = Config::load(p)?;
    let secrets = loa_core::secrets::open_default()?;
    let exe = std::env::current_exe().map_err(|e| LoaError::other(e.to_string()))?;
    let gate = task::TaskGate {
        paths: p.clone(),
        id: id.to_string(),
        exe,
        approval_timeout: cfg.approval_timeout(),
    };
    let mut ctx = Ctx::new(
        p.clone(),
        cfg,
        req.business_id.clone(),
        secrets.as_ref(),
        &gate,
    );
    ctx.dry_run = req.dry_run;
    ctx.cwd = req.cwd.clone().into();
    ctx.task_id = Some(id.to_string());
    actions::execute(&mut ctx, &req.command)
}

// ---------------------------------------------------------------- local commands

fn setup() -> Outcome {
    let p = paths()?;
    let mut cfg = Config::load(&p)?;
    let bin = loa_core::browser::resolve_binary(cfg.agent_browser_path.as_deref())?;
    let ver = loa_core::browser::check_version(&bin)?;
    eprintln!("agent-browser {ver} を確認しました。ブラウザを準備しています…");
    let status = std::process::Command::new(&bin)
        .arg("install")
        .stdout(std::process::Stdio::null())
        .status();
    let browser_ok = status.map(|s| s.success()).unwrap_or(false);
    let secrets = loa_core::secrets::open_default()?;
    loa_core::secrets::get_or_create_key(
        secrets.as_ref(),
        loa_core::secrets::KEY_BROWSER_ENCRYPTION,
        32,
    )?;
    loa_core::secrets::get_or_create_key(secrets.as_ref(), loa_core::secrets::KEY_TERMS_MAC, 32)?;
    loa_core::paths::write_atomic(
        &p.browser_config(),
        loa_core::browser::default_config_json().as_bytes(),
    )?;
    if cfg.default_business_id.is_none() {
        cfg.default_business_id = Some(loa_core::config::DEFAULT_BUSINESS_ID.into());
    }
    cfg.save(&p)?;
    let mut env = Envelope::new(
        "setup",
        "ready",
        json!({
            "agent_browser": {"path": bin.display().to_string(), "version": ver.to_string()},
            "browser_installed": browser_ok,
            "keychain": "ok",
            "data_dir": p.root.display().to_string(),
        }),
    )
    .with_next("loa login");
    if !browser_ok {
        env = env.with_warning(
            "agent-browser install に失敗しました。ネットワークを確認して再実行してください",
        );
    }
    Ok(env)
}

fn terms_ls() -> Outcome {
    let p = paths()?;
    let secrets = loa_core::secrets::open_default()?;
    let store = loa_core::terms::TermsStore::open(&p, secrets.as_ref())?;
    let rows: Vec<serde_json::Value> = store
        .records()?
        .iter()
        .map(|r| {
            json!({
                "terms_id": r.terms_id,
                "name": r.name,
                "approved_at": r.approved_at,
                "hash": &r.hash[..12.min(r.hash.len())],
                "valid": store.is_valid(r),
            })
        })
        .collect();
    Ok(Envelope::new(
        "terms",
        "listed",
        serde_json::Value::Array(rows),
    ))
}

fn scenarios(c: ScenariosCmd) -> Outcome {
    let p = paths()?;
    let mut cfg = Config::load(&p)?;
    match c {
        ScenariosCmd::Update => {
            if let Some(pin) = &cfg.scenarios_pin {
                return Err(
                    LoaError::usage(format!("シナリオは {pin} に固定されています"))
                        .with_next("loa scenarios pin none"),
                );
            }
            let v = loa_core::bundle::update(&p, None)?;
            Ok(Envelope::new("scenarios", "updated", json!({"version": v})))
        }
        ScenariosCmd::List => {
            let cat = loa_core::scenario::Catalog::load(&p, cfg.scenarios_pin.as_deref())?;
            let rows: Vec<serde_json::Value> = cat
                .list()
                .map(|(s, src)| {
                    json!({"id": s.id, "version": s.version, "operation": s.operation, "status": s.status, "source": src.label()})
                })
                .collect();
            let mut env = Envelope::new(
                "scenarios",
                "listed",
                json!({"bundle": cat.bundle_version, "pinned": cfg.scenarios_pin, "installed": loa_core::bundle::installed_versions(&p), "scenarios": rows}),
            );
            env.warnings.extend(cat.warnings.clone());
            Ok(env)
        }
        ScenariosCmd::Pin { version } => {
            if version == "none" {
                cfg.scenarios_pin = None;
                cfg.save(&p)?;
                return Ok(Envelope::new("scenarios", "unpinned", json!({})));
            }
            let looks_like_version = !version.is_empty()
                && version.chars().next().is_some_and(|c| c.is_ascii_digit())
                && version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'));
            if !looks_like_version {
                return Err(LoaError::usage(format!(
                    "バージョンの形式が不正です: {version}（例: 1.4.0）"
                )));
            }
            if !loa_core::bundle::installed_versions(&p).contains(&version) {
                loa_core::bundle::update(&p, Some(&version))?;
            }
            cfg.scenarios_pin = Some(version.clone());
            cfg.save(&p)?;
            Ok(Envelope::new(
                "scenarios",
                "pinned",
                json!({"version": version}),
            ))
        }
    }
}

fn local_ctx<'a>(
    p: &Paths,
    bid: Option<&str>,
    secrets: &'a dyn loa_core::secrets::SecretStore,
    gate: &'a NoHumanGate,
) -> Result<Ctx<'a>> {
    let cfg = Config::load(p)?;
    let business_id = cfg.business_id(bid);
    Ok(Ctx::new(p.clone(), cfg, business_id, secrets, gate))
}

fn mappings(
    maps: &[String],
    hit: &loa_core::resources::ChannelHit,
) -> Result<Vec<actions::envmap::Mapping>> {
    if maps.is_empty() {
        Ok(actions::envmap::default_maps(hit))
    } else {
        maps.iter().map(|m| actions::envmap::parse_map(m)).collect()
    }
}

fn env_write(
    bid: Option<&str>,
    chan: &str,
    provider: Option<&str>,
    out: &str,
    maps: &[String],
) -> Outcome {
    let p = paths()?;
    let secrets = loa_core::secrets::open_default()?;
    let gate = NoHumanGate;
    let mut ctx = local_ctx(&p, bid, secrets.as_ref(), &gate)?;
    let hit = ctx.find_channel(chan, provider)?;
    let maps = mappings(maps, &hit)?;
    let values = actions::envmap::resolve(&mut ctx, &hit, &maps)?;
    let path = ctx.resolve_path(out);
    let written = actions::envmap::write_env_file(&path, &values)?;
    let mut env = Envelope::new(
        "env",
        "written",
        json!({"file": path.display().to_string(), "keys": written}),
    );
    if actions::envmap::git_unignored(&path) {
        env = env.with_warning(format!(
            "{} は .gitignore に含まれていません。秘密がコミットされないよう追加してください",
            path.display()
        ));
    }
    Ok(env)
}

fn exec(bid: Option<&str>, a: &ExecArgs) -> Result<i32> {
    let p = paths()?;
    let secrets = loa_core::secrets::open_default()?;
    let gate = NoHumanGate;
    let mut ctx = local_ctx(&p, bid, secrets.as_ref(), &gate)?;
    let hit = ctx.find_channel(&a.channel, a.provider.as_deref())?;
    let maps = mappings(&a.maps, &hit)?;
    let values = actions::envmap::resolve(&mut ctx, &hit, &maps)?;
    let (prog, rest) = a
        .command
        .split_first()
        .ok_or_else(|| LoaError::usage("実行するコマンドを -- のあとに指定してください"))?;
    let mut c = std::process::Command::new(prog);
    c.args(rest);
    for (k, v) in &values {
        c.env(k, v.as_str());
    }
    let status = c
        .status()
        .map_err(|e| LoaError::usage(format!("{prog} を実行できません: {e}")))?;
    Ok(status.code().unwrap_or(1))
}
