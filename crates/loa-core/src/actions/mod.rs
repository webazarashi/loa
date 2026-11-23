//! Command implementations. Each command runs inside the worker with a [`Ctx`].

pub mod channel;
pub mod declarative;
pub mod envmap;
pub mod liff;
pub mod miniapp;
pub mod oa;
pub mod provider;
pub mod selftest;
pub mod session;
pub mod webhook;

use crate::browser::{AgentBrowser, AgentBrowserConfig, Browser};
use crate::config::Config;
use crate::error::{Code, LoaError, Result};
use crate::gate::{ApprovalRequest, HumanGate};
use crate::ops::{OpKind, PolicyContext};
use crate::output::Envelope;
use crate::paths::Paths;
use crate::resources::{ChannelHit, ResourceMap};
use crate::scenario::{Catalog, Outputs, Runner, Vars};
use crate::secrets::{KEY_BROWSER_ENCRYPTION, SecretRef, SecretStore, get_or_create_key};
use crate::terms::TermsStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use zeroize::Zeroizing;

/// Everything the worker can execute. Serialized into `request.json`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Login,
    Whoami,
    Logout,
    OaList,
    OaGet {
        oa: String,
    },
    OaCreate(oa::CreateArgs),
    OaUpdate(oa::UpdateArgs),
    OaEnableMessagingApi {
        oa: String,
        provider: String,
    },
    ProviderList,
    ProviderGet {
        provider: String,
    },
    ProviderCreate {
        name: String,
    },
    ProviderEnsure {
        name: String,
    },
    ProviderUpdate {
        provider: String,
        new_name: String,
    },
    ChannelList {
        provider: String,
    },
    ChannelGet {
        channel: String,
        provider: Option<String>,
    },
    ChannelCreate(channel::CreateArgs),
    ChannelEnsure(channel::CreateArgs),
    ChannelUpdate(channel::UpdateArgs),
    ChannelRm {
        channel: String,
        provider: Option<String>,
    },
    LiffList {
        channel: String,
        provider: Option<String>,
    },
    LiffGet {
        channel: String,
        provider: Option<String>,
        liff_id: String,
    },
    LiffAdd(liff::AddArgs),
    LiffEnsure(liff::AddArgs),
    LiffUpdate(liff::UpdateArgs),
    LiffRm {
        channel: String,
        provider: Option<String>,
        liff_id: String,
    },
    MiniappGet {
        channel: String,
        provider: Option<String>,
    },
    MiniappUpdate(miniapp::UpdateArgs),
    WebhookGet {
        channel: String,
        provider: Option<String>,
    },
    WebhookSet {
        channel: String,
        provider: Option<String>,
        url: String,
    },
    WebhookTest {
        channel: String,
        provider: Option<String>,
        url: Option<String>,
    },
    Plan {
        file: String,
    },
    Apply {
        file: String,
        prune: bool,
    },
    Selftest {
        provider: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Request {
    pub command: Command,
    pub business_id: String,
    #[serde(default)]
    pub dry_run: bool,
    /// Working directory of the caller (relative paths in arguments resolve against it).
    pub cwd: String,
}

pub struct Ctx<'a> {
    pub paths: Paths,
    pub config: Config,
    pub business_id: String,
    pub secrets: &'a dyn SecretStore,
    pub gate: &'a dyn HumanGate,
    pub dry_run: bool,
    pub cwd: std::path::PathBuf,
    /// Task this context runs in; recorded as the business-lock holder so `BUSY` can point to it.
    pub task_id: Option<String>,
    catalog: Option<Catalog>,
    terms: Option<TermsStore>,
    browser: Option<Box<dyn Browser>>,
    /// Provider for which a bulk approval was given (selftest only).
    bulk_approved_provider: Option<String>,
    pub warnings: Vec<String>,
}

impl<'a> Ctx<'a> {
    pub fn new(
        paths: Paths,
        config: Config,
        business_id: String,
        secrets: &'a dyn SecretStore,
        gate: &'a dyn HumanGate,
    ) -> Self {
        Ctx {
            paths,
            config,
            business_id,
            secrets,
            gate,
            dry_run: false,
            cwd: std::env::current_dir().unwrap_or_default(),
            task_id: None,
            catalog: None,
            terms: None,
            browser: None,
            bulk_approved_provider: None,
            warnings: Vec::new(),
        }
    }

    /// Use a prepared browser (tests).
    pub fn with_browser(mut self, b: Box<dyn Browser>) -> Self {
        self.browser = Some(b);
        self
    }

    pub fn catalog(&mut self) -> Result<&Catalog> {
        if self.catalog.is_none() {
            let c = Catalog::load(&self.paths, self.config.scenarios_pin.as_deref())?;
            self.warnings.extend(c.warnings.iter().cloned());
            self.catalog = Some(c);
        }
        Ok(self.catalog.as_ref().expect("catalog"))
    }

    fn terms(&mut self) -> Result<&TermsStore> {
        if self.terms.is_none() {
            self.terms = Some(TermsStore::open(&self.paths, self.secrets)?);
        }
        Ok(self.terms.as_ref().expect("terms"))
    }

    pub fn browser(&mut self) -> Result<&mut dyn Browser> {
        if self.browser.is_none() {
            crate::task::business_lock(&self.paths, &self.business_id, self.task_id.as_deref())?;
            let bin = crate::browser::resolve_binary(self.config.agent_browser_path.as_deref())?;
            crate::browser::check_version(&bin)?;
            self.paths.ensure()?;
            if !self.paths.browser_config().exists() {
                crate::paths::write_atomic(
                    &self.paths.browser_config(),
                    crate::browser::default_config_json().as_bytes(),
                )?;
            }
            let key = get_or_create_key(self.secrets, KEY_BROWSER_ENCRYPTION, 32)?;
            #[cfg(feature = "test-hooks")]
            let headed = std::env::var("LOA_TEST_HEADLESS").is_err();
            #[cfg(not(feature = "test-hooks"))]
            let headed = true;
            #[cfg(feature = "test-hooks")]
            let namespace =
                std::env::var("LOA_TEST_NAMESPACE").unwrap_or_else(|_| "loa-test".into());
            #[cfg(not(feature = "test-hooks"))]
            let namespace = crate::browser::NAMESPACE.to_string();
            let cfg = AgentBrowserConfig {
                bin,
                session: format!("loa-{}", self.business_id),
                namespace,
                headed,
                encryption_key: key,
                config_file: self.paths.browser_config(),
                cwd: self.paths.browser_dir(),
                idle_timeout: "30m".into(),
                default_timeout_ms: 20_000,
            };
            self.browser = Some(Box::new(AgentBrowser::new(cfg)));
        }
        Ok(self.browser.as_deref_mut().expect("browser"))
    }

    /// Run a scenario by id for an operation (the scenario must declare the same operation).
    pub fn scenario(&mut self, id: &str, op: OpKind, inputs: Vars) -> Result<Outputs> {
        let sc = self.catalog()?.get(id)?.clone();
        let human_timeout = self.config.human_timeout();
        let reports = self.paths.reports_dir();
        self.terms()?;
        self.browser()?;
        let gate = self.gate;
        let terms = self.terms.as_ref().expect("terms");
        let browser = self.browser.as_deref_mut().expect("browser");
        let mut runner = Runner::new(browser, gate, terms, op, reports);
        runner.human_timeout = human_timeout;
        let out = runner.run(&sc, &inputs)?;
        if sc.status == crate::scenario::Status::Provisional {
            let w = format!(
                "シナリオ {id} は実画面で未確認（provisional）です。結果を確認してください"
            );
            if !self.warnings.contains(&w) {
                self.warnings.push(w);
            }
        }
        Ok(out)
    }

    pub fn resources(&self) -> Result<ResourceMap> {
        ResourceMap::load(&self.paths)
    }

    pub fn save_resources(&self, r: &ResourceMap) -> Result<()> {
        r.save(&self.paths)
    }

    /// Gate an operation by the Rust-side policy (spec 4-2).
    pub fn approve(
        &mut self,
        op: OpKind,
        policy: &PolicyContext,
        provider: Option<&str>,
        req: ApprovalRequest,
    ) -> Result<()> {
        if !op.requires_approval(policy) || self.dry_run {
            return Ok(());
        }
        if let (Some(bulk), Some(p)) = (&self.bulk_approved_provider, provider)
            && bulk == p
        {
            return Ok(());
        }
        if self.gate.approve(&req)? {
            Ok(())
        } else {
            Err(LoaError::new(
                Code::ApprovalDenied,
                format!("承認されませんでした: {}", req.title),
            ))
        }
    }

    pub(crate) fn set_bulk_approved(&mut self, provider: &str) {
        self.bulk_approved_provider = Some(provider.to_string());
    }

    pub fn api(&self) -> Result<crate::line_api::LineApi> {
        crate::line_api::LineApi::new()
    }

    /// Channel by name or id, refreshing the local map from the console if needed.
    pub fn find_channel(&mut self, key: &str, provider: Option<&str>) -> Result<ChannelHit> {
        let res = self.resources()?;
        if let Some(b) = res.business_ref(&self.business_id)
            && let Some(hit) = b.find_channel(key, provider)?
        {
            return Ok(hit);
        }
        Err(LoaError::usage(format!("チャネル {key} が見つかりません"))
            .with_hint("loa channel ls --provider <provider> で一覧を更新してから実行してください")
            .with_next("loa channel ls --provider <provider>"))
    }

    pub fn secret_ref(hit: &ChannelHit, key: &str) -> SecretRef {
        SecretRef::new(&hit.provider, &hit.name, key)
    }

    pub fn channel_secret(&self, hit: &ChannelHit) -> Result<Zeroizing<String>> {
        let r = Self::secret_ref(hit, "channel_secret");
        self.secrets.get(&r.store_key())?.ok_or_else(|| {
            LoaError::other(format!(
                "{} のチャネルシークレットが保存されていません",
                hit.name
            ))
            .with_next(format!(
                "loa channel get {} --provider {}",
                hit.name, hit.provider
            ))
        })
    }

    /// Stateless token for API calls (issued per use, never stored).
    pub fn channel_token(&self, hit: &ChannelHit) -> Result<Zeroizing<String>> {
        let secret = self.channel_secret(hit)?;
        self.api()?.issue_stateless_token(&hit.rec.id, &secret)
    }

    pub fn resolve_path(&self, p: &str) -> std::path::PathBuf {
        let pb = std::path::PathBuf::from(p);
        if pb.is_absolute() {
            pb
        } else {
            self.cwd.join(pb)
        }
    }

    pub fn finish(&mut self, mut env: Envelope) -> Envelope {
        for w in self.warnings.drain(..) {
            if !env.warnings.contains(&w) {
                env.warnings.push(w);
            }
        }
        env
    }
}

pub fn planned(resource: &str, what: serde_json::Value) -> Envelope {
    Envelope::new(resource, "planned", what)
}

pub fn execute(ctx: &mut Ctx<'_>, cmd: &Command) -> Result<Envelope> {
    let env = match cmd {
        Command::Login => session::login(ctx)?,
        Command::Whoami => session::whoami(ctx)?,
        Command::Logout => session::logout(ctx)?,
        Command::OaList => oa::list(ctx)?,
        Command::OaGet { oa } => oa::get(ctx, oa)?,
        Command::OaCreate(a) => oa::create(ctx, a)?,
        Command::OaUpdate(a) => oa::update(ctx, a)?,
        Command::OaEnableMessagingApi { oa, provider } => {
            oa::enable_messaging_api(ctx, oa, provider)?
        }
        Command::ProviderList => provider::list(ctx)?,
        Command::ProviderGet { provider } => provider::get(ctx, provider)?,
        Command::ProviderCreate { name } => provider::create(ctx, name, false)?,
        Command::ProviderEnsure { name } => provider::create(ctx, name, true)?,
        Command::ProviderUpdate { provider, new_name } => {
            provider::update(ctx, provider, new_name)?
        }
        Command::ChannelList { provider } => channel::list(ctx, provider)?,
        Command::ChannelGet { channel, provider } => {
            channel::get(ctx, channel, provider.as_deref())?
        }
        Command::ChannelCreate(a) => channel::create(ctx, a, false)?,
        Command::ChannelEnsure(a) => channel::create(ctx, a, true)?,
        Command::ChannelUpdate(a) => channel::update(ctx, a)?,
        Command::ChannelRm { channel, provider } => {
            channel::remove(ctx, channel, provider.as_deref())?
        }
        Command::LiffList { channel, provider } => liff::list(ctx, channel, provider.as_deref())?,
        Command::LiffGet {
            channel,
            provider,
            liff_id,
        } => liff::get(ctx, channel, provider.as_deref(), liff_id)?,
        Command::LiffAdd(a) => liff::add(ctx, a, false)?,
        Command::LiffEnsure(a) => liff::add(ctx, a, true)?,
        Command::LiffUpdate(a) => liff::update(ctx, a)?,
        Command::LiffRm {
            channel,
            provider,
            liff_id,
        } => liff::remove(ctx, channel, provider.as_deref(), liff_id)?,
        Command::MiniappGet { channel, provider } => {
            miniapp::get(ctx, channel, provider.as_deref())?
        }
        Command::MiniappUpdate(a) => miniapp::update(ctx, a)?,
        Command::WebhookGet { channel, provider } => {
            webhook::get(ctx, channel, provider.as_deref())?
        }
        Command::WebhookSet {
            channel,
            provider,
            url,
        } => webhook::set(ctx, channel, provider.as_deref(), url)?,
        Command::WebhookTest {
            channel,
            provider,
            url,
        } => webhook::test(ctx, channel, provider.as_deref(), url.as_deref())?,
        Command::Plan { file } => declarative::plan(ctx, file)?,
        Command::Apply { file, prune } => declarative::apply(ctx, file, *prune)?,
        Command::Selftest { provider } => selftest::run(ctx, provider)?,
    };
    Ok(ctx.finish(env))
}

/// Seconds to wait for scenarios in tests.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(15);

pub(crate) fn vars(pairs: &[(&str, &str)]) -> Vars {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}
