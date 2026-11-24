//! End-to-end tests: the real `loa` binary drives a real (headless) agent-browser against the
//! fake console in `tests/fixtures/fake_line.py`. Never touches LINE.
//!
//!     cargo test -p loa-cli --features test-hooks --test e2e -- --ignored --test-threads=1
//!
//! Needs `python3` and `agent-browser` on PATH.
#![cfg(feature = "test-hooks")]

use serde_json::Value;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

struct Fake {
    child: Child,
    port: u16,
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// `auto_login_ms < 0`: the login page waits for a click (the test logs in through `Env::human`).
fn start_fake(auto_login_ms: i64) -> Fake {
    let script = repo_root().join("tests/fixtures/fake_line.py");
    let mut child = Command::new("python3")
        .arg(script)
        .args(["--port", "0", "--auto-login-ms", &auto_login_ms.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("python3");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let port = line
        .trim()
        .strip_prefix("PORT ")
        .expect("PORT line")
        .parse()
        .unwrap();
    Fake { child, port }
}

struct Env {
    fake: Fake,
    home: PathBuf,
    namespace: String,
    approve: std::cell::RefCell<String>,
}

impl Env {
    fn new(tag: &str) -> Env {
        Env::with_auto_login(tag, 5000)
    }

    fn with_auto_login(tag: &str, auto_login_ms: i64) -> Env {
        let fake = start_fake(auto_login_ms);
        let home = std::env::temp_dir().join(format!("loa-e2e-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        Env {
            fake,
            home,
            namespace: format!("loa-e2e-{tag}-{}", std::process::id()),
            approve: std::cell::RefCell::new("approve".into()),
        }
    }

    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.fake.port)
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let b = self.base();
        let mut c = Command::new(env!("CARGO_BIN_EXE_loa"));
        c.args(args)
            .current_dir(&self.home)
            .env("LOA_HOME", self.home.join("data"))
            .env("LOA_TEST_SECRET_FILE", self.home.join("secrets.json"))
            .env("LOA_TEST_HEADLESS", "1")
            .env("LOA_TEST_NO_UI", "1")
            .env("LOA_TEST_NAMESPACE", &self.namespace)
            .env("LOA_TEST_ALLOW_HOSTS", "127.0.0.1")
            .env("LOA_TEST_APPROVE", self.approve.borrow().as_str())
            .env("LOA_TEST_API_BASE", format!("{b}/api"))
            .env(
                "LOA_TEST_HOST_MAP",
                format!(
                    "developers.line.biz={b}/dev,manager.line.biz={b}/oam,entry.line.biz={b}/entry,account.line.biz={b}/acct"
                ),
            )
            .env_remove("LOA_BUSINESS_ID")
            .stdin(Stdio::null());
        c
    }

    /// Run `loa --json ...`; returns (exit code, parsed stdout, raw stdout+stderr).
    fn run(&self, args: &[&str]) -> (i32, Value, String) {
        let mut full = vec!["--json"];
        full.extend_from_slice(args);
        let out = self.cmd(&full).output().expect("run loa");
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(
            lines.len(),
            1,
            "--json must print exactly one JSON line, got: {stdout}\nstderr: {stderr}"
        );
        let v: Value =
            serde_json::from_str(lines[0]).unwrap_or_else(|e| panic!("not JSON ({e}): {stdout}"));
        (
            out.status.code().unwrap_or(-1),
            v,
            format!("{stdout}\n{stderr}"),
        )
    }

    /// Like `run`, but follows a task through `loa wait` when it stops for a human.
    fn run_done(&self, args: &[&str]) -> (i32, Value, String) {
        let (mut code, mut v, mut raw) = self.run(args);
        for _ in 0..5 {
            if !matches!(code, 10..=12) {
                break;
            }
            let task = v["task"]
                .as_str()
                .expect("waiting error carries task")
                .to_string();
            let (c, w, r) = self.run(&["wait", &task, "--timeout", "120"]);
            code = c;
            v = w;
            raw.push_str(&r);
        }
        (code, v, raw)
    }

    fn set_approve(&self, answer: &str) {
        *self.approve.borrow_mut() = answer.into();
    }

    /// Act as the person in front of the browser (same session the worker drives).
    fn human(&self, args: &[&str]) {
        let st = Command::new("agent-browser")
            .args(["--namespace", &self.namespace, "--session", "loa-default"])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success(), "agent-browser {args:?}");
    }

    fn last_approval(&self) -> String {
        let tasks = self.home.join("data/state/tasks");
        let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;
        for d in std::fs::read_dir(&tasks).into_iter().flatten().flatten() {
            let f = d.path().join("approval-shown.json");
            if let Ok(m) = f.metadata().and_then(|m| m.modified())
                && newest.as_ref().is_none_or(|(t, _)| m > *t)
            {
                newest = Some((m, f));
            }
        }
        newest
            .map(|(_, f)| std::fs::read_to_string(f).unwrap())
            .unwrap_or_default()
    }

    fn state(&self) -> Value {
        let out = Command::new("curl")
            .args(["-s", &format!("{}/__state", self.base())])
            .output()
            .unwrap();
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// The OA update scenarios end on the save click and do not wait for the page to confirm it
    /// (the confirmation on the real screen is not known yet), so the form post can land just
    /// after loa exits. Give the fake a moment before reading.
    fn state_until(&self, done: impl Fn(&Value) -> bool) -> Value {
        for _ in 0..50 {
            let st = self.state();
            if done(&st) {
                return st;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        self.state()
    }

    fn post(&self, path: &str, body: &str) {
        let st = Command::new("curl")
            .args([
                "-s",
                "-X",
                "POST",
                "--data-binary",
                body,
                &format!("{}{path}", self.base()),
            ])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(st.success());
    }

    fn secrets_file(&self) -> String {
        std::fs::read_to_string(self.home.join("secrets.json")).unwrap_or_default()
    }

    fn all_task_files(&self) -> String {
        let mut s = String::new();
        let tasks = self.home.join("data/state/tasks");
        if let Ok(rd) = std::fs::read_dir(&tasks) {
            for d in rd.flatten() {
                for f in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
                    s.push_str(&std::fs::read_to_string(f.path()).unwrap_or_default());
                }
            }
        }
        for name in ["state/resources.json", "state/terms.json", "config.toml"] {
            let f = self.home.join("data").join(name);
            if name != "config.toml" {
                assert!(f.is_file(), "missing {}", f.display());
            }
            s.push_str(&std::fs::read_to_string(f).unwrap_or_default());
        }
        s
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let ns = self.namespace.as_str();
        for args in [
            ["--namespace", ns, "--session", "loa-default", "close"].as_slice(),
            // the saved (fake) login state lives in agent-browser's storage, not in `home`
            ["--namespace", ns, "state", "clear", "--all"].as_slice(),
        ] {
            let _ = Command::new("agent-browser")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
        if std::env::var("LOA_E2E_KEEP").is_err() {
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }
}

fn secret_of(state: &Value, channel_id: &str) -> String {
    state["channels"][channel_id]["secret"]
        .as_str()
        .unwrap()
        .to_string()
}

fn assert_code(r: &(i32, Value, String), code: i32) {
    assert_eq!(r.0, code, "exit code\n{}\n{}", r.1, r.2);
}

#[test]
#[ignore = "launches a browser; run with --ignored"]
fn console_flow() {
    // No timed auto-login: on a slow runner it could log in before loa first looks at the page.
    let env = Env::with_auto_login("flow", -1);

    // 1. first browser command meets the login page: --json returns LOGIN_REQUIRED + task,
    //    the person logs in, `loa wait` follows the same task to the end
    let r = env.run(&["provider", "create", "--name", "loa-e2e"]);
    assert_code(&r, 10);
    assert_eq!(r.1["code"], "LOGIN_REQUIRED");
    assert!(r.1["next"][0].as_str().unwrap().starts_with("loa wait"));
    let task = r.1["task"].as_str().unwrap().to_string();
    env.human(&[
        "find",
        "role",
        "link",
        "click",
        "--name",
        "ログイン",
        "--exact",
    ]);
    let r = env.run(&["wait", &task, "--timeout", "120"]);
    assert_code(&r, 0);
    let pid = r.1["data"]["id"].as_str().unwrap().to_string();
    assert_eq!(env.state()["providers"][&pid]["name"], "loa-e2e");

    // 2. logged in now
    let r = env.run(&["whoami"]);
    assert_code(&r, 0);
    assert_eq!(r.1["data"]["logged_in"], true, "{}", r.1);
    assert_eq!(r.1["data"]["account"], "テスト 太郎", "{}", r.1);

    // 3. ensure is a no-op when it exists
    let r = env.run(&["provider", "ensure", "--name", "loa-e2e"]);
    assert_code(&r, 0);
    assert_eq!(r.1["action"], "unchanged");
    assert_eq!(env.state()["providers"].as_object().unwrap().len(), 1);

    // 4. dry-run changes nothing
    let r = env.run(&["--dry-run", "provider", "create", "--name", "loa-e2e-dry"]);
    assert_code(&r, 0);
    assert_eq!(env.state()["providers"].as_object().unwrap().len(), 1);

    // 5. login channel: terms approval, consent dialog, secret goes to the store only
    let r = env.run_done(&[
        "channel",
        "create",
        "--provider",
        "loa-e2e",
        "--type",
        "login",
        "--name",
        "e2e-login",
        "--description",
        "e2e channel",
        "--callback-url",
        "https://example.com/cb",
    ]);
    assert_code(&r, 0);
    let cid = r.1["data"]["id"].as_str().unwrap().to_string();
    let st = env.state();
    assert_eq!(st["channels"][&cid]["name"], "e2e-login");
    assert_eq!(st["channels"][&cid]["consent"], true);
    assert_eq!(st["channels"][&cid]["callbacks"], "https://example.com/cb");
    let secret = secret_of(&st, &cid);
    assert!(!r.2.contains(&secret), "channel secret leaked to output");
    assert!(
        env.secrets_file().contains(&secret),
        "secret must be in the store"
    );
    assert!(
        !env.all_task_files().contains(&secret),
        "secret leaked to state files"
    );
    // secrets are keyed by channel id: names are not unique and can change
    assert_eq!(
        r.1["data"]["secret_ref"],
        format!("loa://loa-e2e/{cid}/channel_secret")
    );

    // messaging channels are created from an official account, not here
    let r = env.run(&[
        "channel",
        "create",
        "--provider",
        "loa-e2e",
        "--type",
        "messaging",
        "--name",
        "x",
    ]);
    assert_code(&r, 2);
    assert!(
        r.1["next"][0]
            .as_str()
            .unwrap()
            .contains("enable-messaging-api"),
        "{}",
        r.1
    );

    // terms were recorded once, with a valid MAC
    let r = env.run(&["terms", "ls"]);
    assert_code(&r, 0);
    assert!(
        r.1["data"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["terms_id"] == "line-developers-agreement" && t["valid"] == true),
        "{}",
        r.1
    );

    // 6. LIFF through the mock API
    let r = env.run(&[
        "liff",
        "add",
        "--channel",
        "e2e-login",
        "--name",
        "main",
        "--url",
        "https://example.com/liff",
    ]);
    assert_code(&r, 0);
    let liff_id = r.1["data"]["liff_id"].as_str().unwrap().to_string();
    let r = env.run(&[
        "liff",
        "ensure",
        "--channel",
        "e2e-login",
        "--name",
        "main",
        "--url",
        "https://example.com/liff2",
    ]);
    assert_code(&r, 0);
    assert_eq!(
        env.state()["channels"][&cid]["liff"][&liff_id]["view"]["url"],
        "https://example.com/liff2"
    );
    let r = env.run(&["liff", "get", &liff_id]);
    assert_code(&r, 0);
    assert!(r.2.contains("https://example.com/liff2"));

    // 7. env write: values only in the file, key names on stdout
    let r = env.run(&[
        "env",
        "write",
        "--channel",
        "e2e-login",
        "--out",
        ".env.local",
    ]);
    assert_code(&r, 0);
    let file = std::fs::read_to_string(env.home.join(".env.local")).unwrap();
    assert!(
        file.contains(&secret),
        "env file must hold the secret: {file}"
    );
    assert!(
        file.contains(&liff_id),
        "env file must hold the LIFF ID: {file}"
    );
    assert!(!r.2.contains(&secret));

    // 8. exec passes values through the environment only
    let out = env
        .cmd(&[
            "exec",
            "--channel",
            "e2e-login",
            "--map",
            "S=channel.secret",
            "--",
            "sh",
            "-c",
            "test \"$S\" = \"$EXPECT\" && echo same",
        ])
        .env("EXPECT", &secret)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "same",
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // 9. basic settings through the console
    let r = env.run_done(&["channel", "update", "e2e-login", "--description", "changed"]);
    assert_code(&r, 0);
    assert_eq!(env.state()["channels"][&cid]["description"], "changed");

    // 10. LIFF delete needs approval: denied changes nothing, approved deletes
    env.set_approve("deny");
    let r = env.run_done(&["liff", "rm", &liff_id]);
    assert_code(&r, 13);
    assert!(
        env.state()["channels"][&cid]["liff"]
            .get(&liff_id)
            .is_some()
    );
    env.set_approve("approve");
    let r = env.run_done(&["liff", "rm", &liff_id]);
    assert_code(&r, 0);
    assert!(
        env.state()["channels"][&cid]["liff"]
            .get(&liff_id)
            .is_none()
    );

    // 11. MINI App: separate terms, three internal stages, review lock
    let r = env.run_done(&[
        "channel",
        "create",
        "--provider",
        "loa-e2e",
        "--type",
        "miniapp",
        "--name",
        "e2e-mini",
        "--description",
        "mini",
    ]);
    assert_code(&r, 0);
    let mid = r.1["data"]["id"].as_str().unwrap().to_string();
    let r = env.run(&["miniapp", "get", "e2e-mini"]);
    assert_code(&r, 0);
    let st = env.state();
    for stage in ["developing", "review", "published"] {
        assert_eq!(
            r.1["data"]["stages"][stage]["liff_id"],
            st["channels"][&mid]["miniapp"][stage]["liff_id"],
            "{}",
            r.1
        );
    }
    let r = env.run(&[
        "miniapp",
        "update",
        "e2e-mini",
        "--stage",
        "developing",
        "--endpoint-url",
        "https://example.com/dev",
    ]);
    assert_code(&r, 0);
    assert_eq!(
        env.state()["channels"][&mid]["miniapp"]["developing"]["url"],
        "https://example.com/dev"
    );
    env.post(&format!("/__lock?channel={mid}&stage=review"), "");
    let r = env.run(&[
        "miniapp",
        "update",
        "e2e-mini",
        "--stage",
        "review",
        "--endpoint-url",
        "https://example.com/rv",
    ]);
    assert_code(&r, 21);
    assert_eq!(
        env.state()["channels"][&mid]["miniapp"]["review"]["url"],
        ""
    );
    let r = env.run(&[
        "liff",
        "add",
        "--channel",
        "e2e-mini",
        "--name",
        "x",
        "--url",
        "https://example.com/x",
    ]);
    assert_code(&r, 2);

    // 12. channel delete: the dialog shows name and ID; denied keeps it, approved removes it
    env.set_approve("deny");
    let r = env.run_done(&["channel", "rm", "e2e-mini"]);
    assert_code(&r, 13);
    let shown = env.last_approval();
    assert!(
        shown.contains("e2e-mini") && shown.contains(&mid),
        "approval must show target: {shown}"
    );
    assert!(env.state()["channels"].get(&mid).is_some());
    env.set_approve("approve");
    let r = env.run_done(&["channel", "rm", "e2e-mini"]);
    assert_code(&r, 0);
    assert!(env.state()["channels"].get(&mid).is_none());

    // 13. console changed: UI_CHANGED with a local report that holds no secrets
    env.post("/__flag?name=broken&value=1", "");
    let r = env.run(&["provider", "create", "--name", "loa-e2e-2"]);
    assert_code(&r, 20);
    let report = r.1["report"].as_str().expect("report path");
    let issue = std::fs::read_to_string(Path::new(report).join("issue.md")).unwrap();
    assert!(issue.contains("provider.create"), "{issue}");
    assert!(!issue.contains(&secret));
    env.post("/__flag?name=broken&value=0", "");

    // 14. leaving the allowed hosts stops the run
    env.post("/__flag?name=evil&value=1", "");
    let r = env.run(&["provider", "ls"]);
    assert_code(&r, 22);
    env.post("/__flag?name=evil&value=0", "");
}

#[test]
#[ignore = "launches a browser; run with --ignored"]
fn official_account_flow() {
    let env = Env::new("oa");
    // log in once through any page
    let r = env.run_done(&["provider", "create", "--name", "loa-oa"]);
    assert_code(&r, 0);
    // the service's LINE Login channel has the name the official account will get
    let r = env.run_done(&[
        "channel",
        "create",
        "--provider",
        "loa-oa",
        "--type",
        "login",
        "--name",
        "E2E Shop",
        "--description",
        "shop login",
    ]);
    assert_code(&r, 0);
    let login = r.1["data"]["id"].as_str().unwrap().to_string();

    // 1. account creation pauses for the person twice: the form, then a reCAPTCHA image check
    //    after 完了 (here the test plays that role). The confirmation page shows an email address,
    //    so its "@" must not pass for the new basic ID.
    env.post("/__flag?name=recaptcha&value=1", "");
    let r = env.run(&[
        "oa",
        "create",
        "--name",
        "E2E Shop",
        "--company",
        "E2E Inc.",
    ]);
    assert_code(&r, 11);
    let task = r.1["task"].as_str().unwrap().to_string();
    let click_confirm = || {
        env.human(&[
            "find", "role", "button", "click", "--name", "確認", "--exact",
        ])
    };
    click_confirm();
    // `loa wait` follows the task to the end, so watch for the second human wait with short waits
    let asked = (0..60).any(|_| {
        let r = env.run(&["wait", &task, "--timeout", "2"]);
        assert_ne!(r.0, 0, "created without the image check: {}", r.1);
        r.0 == 14
            && r.1["detail"]["hint"]
                .as_str()
                .is_some_and(|h| h.contains("reCAPTCHA"))
    });
    assert!(asked, "the reCAPTCHA check must wait for the person");
    click_confirm();
    let r = env.run(&["wait", &task, "--timeout", "120"]);
    assert_code(&r, 0);
    let oid = r.1["data"]["id"].as_str().unwrap().to_string();
    assert!(oid.starts_with('@') && !oid.contains("example"), "{}", r.1);
    assert_eq!(env.state()["oas"][&oid]["name"], "E2E Shop");
    env.post("/__flag?name=recaptcha&value=0", "");

    // 2. Messaging API: the new account first shows the information-use consent (terms), then
    //    the enable flow (approval + API terms)
    let r = env.run_done(&[
        "oa",
        "enable-messaging-api",
        "E2E Shop",
        "--provider",
        "loa-oa",
    ]);
    assert_code(&r, 0);
    assert_eq!(r.1["action"], "updated");
    let st = env.state();
    assert_eq!(st["oas"][&oid]["consented"], true);
    let ch = st["oas"][&oid]["messaging"]["channel_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(r.1["data"]["channel_id"], ch.as_str());
    assert_eq!(r.1["data"]["channel"], "E2E Shop");
    let secret = secret_of(&st, &ch);
    let login_secret = secret_of(&st, &login);
    assert!(!r.2.contains(&secret));
    let terms = std::fs::read_to_string(env.home.join("data/state/terms.json")).unwrap();
    assert!(
        terms.contains("line-official-account-information-use"),
        "{terms}"
    );

    // already enabled: nothing to approve, the channel is read again (idempotent)
    env.set_approve("deny");
    let r = env.run_done(&[
        "oa",
        "enable-messaging-api",
        "E2E Shop",
        "--provider",
        "loa-oa",
    ]);
    assert_code(&r, 0);
    assert_eq!(r.1["action"], "unchanged");
    assert_eq!(r.1["data"]["channel_id"], ch.as_str());
    env.set_approve("approve");

    // 3. the Messaging API channel and the LINE Login channel share the name "E2E Shop":
    //    each keeps its own record and secret
    let r = env.run_done(&["channel", "get", &ch, "--provider", "loa-oa"]);
    assert_code(&r, 0);
    assert_eq!(r.1["data"]["name"], "E2E Shop");
    assert_eq!(r.1["data"]["type"], "messaging");
    let r = env.run_done(&["channel", "get", &login, "--provider", "loa-oa"]);
    assert_code(&r, 0);
    assert_eq!(r.1["data"]["name"], "E2E Shop");
    assert_eq!(r.1["data"]["type"], "login");
    let r = env.run(&["channel", "get", "E2E Shop", "--provider", "loa-oa"]);
    assert_code(&r, 2);
    for (id, want) in [(&ch, &secret), (&login, &login_secret)] {
        let r = env.run(&[
            "env",
            "write",
            "--channel",
            id,
            "--provider",
            "loa-oa",
            "--out",
            "both.env",
            "--map",
            "S=channel.secret",
            "--map",
            "N=channel.name",
        ]);
        assert_code(&r, 0);
        let file = std::fs::read_to_string(env.home.join("both.env")).unwrap();
        assert!(file.contains(want.as_str()), "{id}: {file}");
        assert!(file.contains("N=\"E2E Shop\""), "{id}: {file}");
        assert!(!r.2.contains(want.as_str()));
    }

    // 4. profile + settings (no approval needed)
    let r = env.run_done(&[
        "oa",
        "update",
        "E2E Shop",
        "--profile.status-message",
        "hello",
        "--settings.join-groups",
        "false",
    ]);
    assert_code(&r, 0);
    let st = env.state_until(|st| {
        st["oas"][&oid]["status_message"] == "hello" && st["oas"][&oid]["join_groups"] == false
    });
    assert_eq!(st["oas"][&oid]["status_message"], "hello");
    assert_eq!(st["oas"][&oid]["join_groups"], false);

    // 5. response mode needs approval
    env.set_approve("deny");
    let r = env.run_done(&["oa", "update", "E2E Shop", "--response.mode", "webhook"]);
    assert_code(&r, 13);
    assert_eq!(env.state()["oas"][&oid]["webhook"], false);
    env.set_approve("approve");
    let r = env.run_done(&["oa", "update", "E2E Shop", "--response.mode", "webhook"]);
    assert_code(&r, 0);
    let st = env.state_until(|st| st["oas"][&oid]["webhook"] == true);
    assert_eq!(st["oas"][&oid]["webhook"], true);
    assert_eq!(st["oas"][&oid]["chat"], false);

    // a denied approval leaves the other requested changes undone too
    env.set_approve("deny");
    let r = env.run_done(&[
        "oa",
        "update",
        "E2E Shop",
        "--profile.status-message",
        "changed-while-denied",
        "--response.mode",
        "chat",
    ]);
    assert_code(&r, 13);
    assert_eq!(env.state()["oas"][&oid]["status_message"], "hello");
    env.set_approve("approve");

    // 6. webhook through the API
    let r = env.run_done(&[
        "webhook",
        "set",
        "--channel",
        &ch,
        "--url",
        "https://example.com/hook",
    ]);
    assert_code(&r, 0);
    assert_eq!(
        env.state()["channels"][&ch]["webhook"],
        "https://example.com/hook"
    );
    // changing an existing URL needs approval
    env.set_approve("deny");
    let r = env.run_done(&[
        "webhook",
        "set",
        "--channel",
        &ch,
        "--url",
        "https://example.com/other",
    ]);
    assert_code(&r, 13);
    assert_eq!(
        env.state()["channels"][&ch]["webhook"],
        "https://example.com/hook"
    );
    env.set_approve("approve");
    let r = env.run(&["webhook", "test", "--channel", &ch]);
    assert_code(&r, 0);

    // 7. long-lived channel access token: issued once, then read again (never reissued
    //    silently); reissuing needs approval
    let r = env.run_done(&["channel", "issue-token", &ch, "--provider", "loa-oa"]);
    assert_code(&r, 0);
    assert_eq!(
        r.1["data"]["token_ref"],
        format!("loa://loa-oa/{ch}/channel_access_token")
    );
    let token = env.state()["channels"][&ch]["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(!r.2.contains(&token), "token leaked to output");
    assert!(env.secrets_file().contains(&token));
    assert!(
        !env.all_task_files().contains(&token),
        "token leaked to state files"
    );
    let r = env.run_done(&["channel", "issue-token", &ch, "--provider", "loa-oa"]);
    assert_code(&r, 0);
    assert_eq!(env.state()["channels"][&ch]["access_token"], token.as_str());
    env.set_approve("deny");
    let r = env.run_done(&[
        "channel",
        "issue-token",
        &ch,
        "--provider",
        "loa-oa",
        "--reissue",
    ]);
    assert_code(&r, 13);
    assert_eq!(env.state()["channels"][&ch]["access_token"], token.as_str());
    env.set_approve("approve");
    let r = env.run_done(&[
        "channel",
        "issue-token",
        &ch,
        "--provider",
        "loa-oa",
        "--reissue",
    ]);
    assert_code(&r, 0);
    let token2 = env.state()["channels"][&ch]["access_token"]
        .as_str()
        .unwrap()
        .to_string();
    assert_ne!(token, token2);
    assert!(!r.2.contains(&token2));
    let r = env.run(&[
        "env",
        "write",
        "--channel",
        &ch,
        "--provider",
        "loa-oa",
        "--out",
        "bot.env",
        "--map",
        "LINE_CHANNEL_ACCESS_TOKEN=channel.access_token",
    ]);
    assert_code(&r, 0);
    let file = std::fs::read_to_string(env.home.join("bot.env")).unwrap();
    assert!(file.contains(&token2), "{file}");
    assert!(!r.2.contains(&token2));
    // LINE Login channels have no such token
    let r = env.run(&["channel", "issue-token", &login, "--provider", "loa-oa"]);
    assert_code(&r, 2);

    // 8. link the official account to the LINE Login channel (friendship option)
    let r = env.run_done(&[
        "channel",
        "update",
        &login,
        "--provider",
        "loa-oa",
        "--linked-oa",
        "E2E Shop",
    ]);
    assert_code(&r, 0);
    assert_eq!(env.state()["channels"][&login]["linked_oa"], oid.as_str());

    // 9. member invite needs approval and returns an invitation URL
    let r = env.run_done(&[
        "oa",
        "update",
        "E2E Shop",
        "--members.add",
        "invite",
        "--members.role",
        "operator",
    ]);
    assert_code(&r, 0);
    assert!(
        r.2.contains("https://manager.line.biz/invitation/"),
        "{}",
        r.1
    );
}

#[test]
#[ignore = "launches a browser; run with --ignored"]
fn declarative_flow() {
    let env = Env::new("decl");
    let yaml = r#"version: 1
providers:
  - name: loa-decl
    channels:
      - name: decl-login
        type: login
        description: declarative
        callback_urls: [https://example.com/cb]
        liff:
          - name: main
            url: https://example.com/app
            size: tall
          - name: sub
            url: https://example.com/sub
"#;
    std::fs::write(env.home.join("loa.yaml"), yaml).unwrap();

    // plan never changes anything (it still logs in and reads the console)
    let r = env.run_done(&["plan"]);
    assert_code(&r, 0);
    assert_eq!(r.1["action"], "planned", "{}", r.1);
    assert!(env.state()["providers"].as_object().unwrap().is_empty());

    // apply creates everything
    let r = env.run_done(&["apply"]);
    assert_code(&r, 0);
    let st = env.state();
    assert_eq!(st["providers"].as_object().unwrap().len(), 1);
    let (cid, ch) = st["channels"].as_object().unwrap().iter().next().unwrap();
    assert_eq!(ch["name"], "decl-login");
    assert_eq!(ch["callbacks"], "https://example.com/cb");
    assert_eq!(ch["liff"].as_object().unwrap().len(), 2, "{ch}");
    let cid = cid.clone();

    // second apply: no changes
    let r = env.run_done(&["apply"]);
    assert_code(&r, 0);
    assert_eq!(r.1["action"], "unchanged", "{}", r.1);

    // drift: change the URL in the file → plan shows one update, apply fixes it
    std::fs::write(
        env.home.join("loa.yaml"),
        yaml.replace("https://example.com/app", "https://example.com/app2"),
    )
    .unwrap();
    let r = env.run_done(&["plan"]);
    assert_code(&r, 0);
    assert!(
        r.1.to_string().contains("https://example.com/app2"),
        "{}",
        r.1
    );
    let r = env.run_done(&["apply"]);
    assert_code(&r, 0);
    let st = env.state();
    assert!(
        st["channels"][&cid]["liff"]
            .as_object()
            .unwrap()
            .values()
            .any(|a| a["view"]["url"] == "https://example.com/app2"),
        "{st}"
    );

    // prune: removing `sub` from the file deletes it only with --prune and approval
    let pruned = yaml.replace(
        "          - name: sub\n            url: https://example.com/sub\n",
        "",
    );
    std::fs::write(env.home.join("loa.yaml"), pruned).unwrap();
    let r = env.run_done(&["apply"]);
    assert_code(&r, 0);
    assert_eq!(
        env.state()["channels"][&cid]["liff"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    env.set_approve("deny");
    let r = env.run_done(&["apply", "--prune"]);
    assert_code(&r, 13);
    assert_eq!(
        env.state()["channels"][&cid]["liff"]
            .as_object()
            .unwrap()
            .len(),
        2
    );
    env.set_approve("approve");
    let r = env.run_done(&["apply", "--prune"]);
    assert_code(&r, 0);
    assert_eq!(
        env.state()["channels"][&cid]["liff"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
}

#[test]
#[ignore = "launches a browser; run with --ignored"]
fn selftest_and_busy() {
    let env = Env::new("self");
    let r = env.run_done(&["selftest"]);
    assert_code(&r, 0);
    assert_eq!(r.1["action"], "passed", "{}", r.1);
    // selftest leaves no channels behind
    let st = env.state();
    assert!(
        st["channels"]
            .as_object()
            .unwrap()
            .values()
            .all(|c| !c["name"].as_str().unwrap().starts_with("loa-selftest-")),
        "{st}"
    );

    // a second browser command for the same business ID while one runs → BUSY
    env.post("/__flag?name=slow&value=1", "");
    let mut first = env
        .cmd(&["provider", "create", "--name", "loa-busy"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let r = env.run(&["provider", "ls"]);
    let _ = first.wait();
    env.post("/__flag?name=slow&value=0", "");
    assert_eq!(r.0, 31, "expected BUSY: {}", r.1);
    assert_eq!(r.1["code"], "BUSY");
}
