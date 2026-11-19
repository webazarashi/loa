//! Tasks: every command that may need a browser or a human runs in a detached worker
//! (`loa __worker <id>`). The client follows `status.json`; `loa wait` re-attaches.
//!
//! Liveness: the worker holds an exclusive OS lock on `worker.lock` for its whole life.
//! If a client can take that lock while the task is unfinished, the worker died (WORKER_LOST).

use crate::error::{Code, LoaError, Result};
use crate::gate::{ApprovalRequest, DoneCheck, HumanGate, HumanKind, PasteRequest};
use crate::output::Outcome;
use crate::paths::{Paths, write_atomic};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Pending,
    Running,
    Waiting,
    Done,
    Failed,
}

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskState::Done | TaskState::Failed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Waiting {
    pub code: Code,
    pub hint: String,
    pub since: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskStatus {
    pub id: String,
    pub state: TaskState,
    #[serde(default)]
    pub waiting: Option<Waiting>,
    #[serde(default)]
    pub progress: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

pub fn new_id() -> String {
    format!("t_{}", crate::util::random_hex(4))
}

pub fn status_path(paths: &Paths, id: &str) -> PathBuf {
    paths.task_dir(id).join("status.json")
}

fn valid_id(id: &str) -> bool {
    id.starts_with("t_") && id.len() <= 32 && id[2..].chars().all(|c| c.is_ascii_hexdigit())
}

pub fn create(paths: &Paths, request: &Value) -> Result<String> {
    gc(paths);
    let id = new_id();
    let dir = paths.task_dir(&id);
    crate::paths::create_private_dir(&dir)?;
    write_atomic(
        &dir.join("request.json"),
        &serde_json::to_vec_pretty(request)?,
    )?;
    let now = crate::util::now_rfc3339();
    write_status(
        paths,
        &TaskStatus {
            id: id.clone(),
            state: TaskState::Pending,
            waiting: None,
            progress: None,
            created_at: now.clone(),
            updated_at: now,
            pid: None,
        },
    )?;
    Ok(id)
}

pub fn read_request(paths: &Paths, id: &str) -> Result<Value> {
    if !valid_id(id) {
        return Err(LoaError::usage(format!("タスクID {id} は不正です")));
    }
    let b = std::fs::read(paths.task_dir(id).join("request.json"))
        .map_err(|_| LoaError::usage(format!("タスク {id} が見つかりません")))?;
    Ok(serde_json::from_slice(&b)?)
}

pub fn read_status(paths: &Paths, id: &str) -> Result<TaskStatus> {
    if !valid_id(id) {
        return Err(LoaError::usage(format!("タスクID {id} は不正です")));
    }
    let b = std::fs::read(status_path(paths, id))
        .map_err(|_| LoaError::usage(format!("タスク {id} が見つかりません")))?;
    Ok(serde_json::from_slice(&b)?)
}

pub fn write_status(paths: &Paths, st: &TaskStatus) -> Result<()> {
    write_atomic(&status_path(paths, &st.id), &serde_json::to_vec_pretty(st)?)
}

pub fn update_status(paths: &Paths, id: &str, f: impl FnOnce(&mut TaskStatus)) -> Result<()> {
    let mut st = read_status(paths, id)?;
    f(&mut st);
    st.updated_at = crate::util::now_rfc3339();
    write_status(paths, &st)
}

/// Result file: the exact JSON the command prints (`{ok:true,...}` or `{ok:false,...}`).
pub fn write_result(paths: &Paths, id: &str, outcome: &Outcome) -> Result<()> {
    let v: Value = match outcome {
        Ok(env) => serde_json::to_value(env)?,
        Err(e) => {
            let mut e = e.clone();
            e.task = Some(id.to_string());
            serde_json::from_str(&crate::output::error_json(&e))?
        }
    };
    let redacted = crate::redact::redact_json(&v);
    write_atomic(
        &paths.task_dir(id).join("result.json"),
        &serde_json::to_vec_pretty(&redacted)?,
    )?;
    update_status(paths, id, |s| {
        s.state = if outcome.is_ok() {
            TaskState::Done
        } else {
            TaskState::Failed
        };
        s.waiting = None;
    })
}

pub fn read_result(paths: &Paths, id: &str) -> Result<Outcome> {
    let b = std::fs::read(paths.task_dir(id).join("result.json"))?;
    let v: Value = serde_json::from_slice(&b)?;
    if v.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(Ok(serde_json::from_value(v)?))
    } else {
        Ok(Err(serde_json::from_value(v)?))
    }
}

/// Hold an exclusive lock on `path` until the process exits. Ok(false) = someone else holds it.
pub fn hold_lock(path: &Path) -> Result<bool> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let f = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    let lock: &'static mut fd_lock::RwLock<std::fs::File> =
        Box::leak(Box::new(fd_lock::RwLock::new(f)));
    match lock.try_write() {
        Ok(guard) => {
            std::mem::forget(guard);
            Ok(true)
        }
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// Is a process holding the lock? (Takes and immediately releases it if free.)
pub fn lock_is_held(path: &Path) -> bool {
    let Ok(f) = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
    else {
        return false;
    };
    let mut lock = fd_lock::RwLock::new(f);
    matches!(lock.try_write(), Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock)
}

pub fn worker_lock_path(paths: &Paths, id: &str) -> PathBuf {
    paths.task_dir(id).join("worker.lock")
}

/// Business-level lock: one browser automation per LINE Business ID at a time.
pub fn business_lock(paths: &Paths, business_id: &str) -> Result<()> {
    let p = paths.locks_dir().join(format!("{business_id}.lock"));
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if hold_lock(&p)? {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(LoaError::new(
                Code::Busy,
                format!("LINEビジネスID {business_id} で別の操作が実行中です"),
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Spawn the detached worker process.
pub fn spawn_worker(exe: &Path, paths: &Paths, id: &str) -> Result<()> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(paths.task_dir(id).join("worker.log"))?;
    let mut c = Command::new(exe);
    c.arg("__worker").arg(id);
    c.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        c.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
        c.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB);
        if c.spawn().is_ok() {
            return Ok(());
        }
        c.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    c.spawn()
        .map(|_| ())
        .map_err(|e| LoaError::other(format!("ワーカーを起動できません: {e}")))
}

/// How the client follows a task.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FollowMode {
    /// `--json` command: return as soon as a human is needed.
    ExitOnHuman,
    /// Human display or `loa wait`: keep waiting.
    UntilDone,
}

pub enum FollowEvent<'a> {
    Progress(&'a str),
    Waiting(&'a Waiting),
}

/// Follow a task until it finishes, needs a human (ExitOnHuman), or `timeout` passes.
pub fn follow(
    paths: &Paths,
    id: &str,
    mode: FollowMode,
    timeout: Option<Duration>,
    mut on_event: impl FnMut(FollowEvent<'_>),
) -> Outcome {
    let start = Instant::now();
    let mut last_progress = String::new();
    let mut last_wait: Option<Waiting> = None;
    let lock = worker_lock_path(paths, id);
    loop {
        let st = read_status(paths, id).map_err(|e| e.with_task(id))?;
        if st.state.is_terminal() {
            return read_result(paths, id)
                .map_err(|e| e.with_task(id))?
                .map_err(|e| e.with_task(id));
        }
        if let Some(p) = &st.progress
            && *p != last_progress
        {
            on_event(FollowEvent::Progress(p));
            last_progress = p.clone();
        }
        if let Some(w) = &st.waiting {
            if last_wait.as_ref() != Some(w) {
                on_event(FollowEvent::Waiting(w));
                last_wait = Some(w.clone());
            }
            if mode == FollowMode::ExitOnHuman {
                return Err(LoaError::new(w.code, w.hint.clone())
                    .with_hint(w.hint.clone())
                    .with_task(id)
                    .with_next(format!("loa wait {id} --timeout 300")));
            }
        }
        let started = st.pid.is_some() || st.state != TaskState::Pending;
        let grace = start.elapsed() > Duration::from_secs(10);
        if (started || grace) && !lock_is_held(&lock) {
            // re-check: the worker may have just finished
            let st2 = read_status(paths, id)?;
            if !st2.state.is_terminal() {
                return Err(LoaError::new(
                    Code::WorkerLost,
                    "タスクを実行していたプロセスが終了しました",
                )
                .with_task(id)
                .with_next(
                    "同じコマンドをもう一度実行してください（作成済みのものは重複作成されません）",
                ));
            }
            continue;
        }
        if let Some(t) = timeout
            && start.elapsed() > t
        {
            let mut e = LoaError::new(
                Code::WaitTimeout,
                "待機時間を過ぎました（タスクは続いています）",
            )
            .with_task(id)
            .with_next(format!("loa wait {id} --timeout 300"));
            if let Some(w) = &st.waiting {
                e = e.with_detail(serde_json::json!({ "waiting": w.code, "hint": w.hint }));
            }
            return Err(e);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Remove finished tasks older than 7 days.
fn gc(paths: &Paths) {
    let Ok(rd) = std::fs::read_dir(paths.tasks_dir()) else {
        return;
    };
    let week = Duration::from_secs(7 * 24 * 3600);
    for e in rd.flatten() {
        let p = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > week);
        if old && p.join("result.json").exists() {
            let _ = std::fs::remove_dir_all(p);
        }
    }
}

// ------------------------------------------------------------------ worker-side gate

/// Gate used inside the worker: publishes waiting states to status.json and shows the
/// `loa __ui` windows. Approval results come back only through the child's stdout pipe.
pub struct TaskGate {
    pub paths: Paths,
    pub id: String,
    pub exe: PathBuf,
    pub approval_timeout: Duration,
}

impl TaskGate {
    fn set_waiting(&self, code: Code, hint: &str) {
        let _ = update_status(&self.paths, &self.id, |s| {
            s.state = TaskState::Waiting;
            s.waiting = Some(Waiting {
                code,
                hint: hint.to_string(),
                since: crate::util::now_rfc3339(),
            });
        });
    }

    fn clear_waiting(&self) {
        let _ = update_status(&self.paths, &self.id, |s| {
            s.state = TaskState::Running;
            s.waiting = None;
        });
    }

    /// Run `loa __ui <kind>` with JSON on stdin; returns stdout (None on timeout / failure).
    fn run_ui(&self, kind: &str, input: &Value, timeout: Duration) -> Option<Zeroizing<String>> {
        let mut child = Command::new(&self.exe)
            .args(["__ui", kind])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(input.to_string().as_bytes());
        }
        let mut stdout = child.stdout.take()?;
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut s = Zeroizing::new(String::new());
            let _ = stdout.read_to_string(&mut s);
            let _ = tx.send(s);
        });
        match rx.recv_timeout(timeout) {
            Ok(s) => {
                let _ = child.wait();
                Some(s)
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                None
            }
        }
    }

    #[cfg(feature = "test-hooks")]
    fn test_answer(&self, var: &str) -> Option<String> {
        std::env::var(var).ok()
    }
}

impl HumanGate for TaskGate {
    fn approve(&self, req: &ApprovalRequest) -> Result<bool> {
        self.set_waiting(Code::ApprovalRequired, &req.title);
        #[cfg(feature = "test-hooks")]
        if let Some(a) = self.test_answer("LOA_TEST_APPROVE") {
            std::thread::sleep(Duration::from_millis(600));
            self.clear_waiting();
            // log what would have been shown, for assertions in tests
            let _ = std::fs::write(
                self.paths.task_dir(&self.id).join("approval-shown.json"),
                serde_json::to_vec_pretty(req).unwrap_or_default(),
            );
            return Ok(a == "approve");
        }
        let input = serde_json::to_value(req)?;
        let out = self.run_ui("approve", &input, self.approval_timeout);
        self.clear_waiting();
        Ok(out.is_some_and(|s| s.trim() == "approved"))
    }

    fn paste(&self, req: &PasteRequest) -> Result<Zeroizing<String>> {
        self.set_waiting(Code::HumanActionRequired, &req.title);
        #[cfg(feature = "test-hooks")]
        if let Some(v) = self.test_answer("LOA_TEST_PASTE") {
            self.clear_waiting();
            return Ok(Zeroizing::new(v));
        }
        let input = serde_json::to_value(req)?;
        let out = self.run_ui("paste", &input, self.approval_timeout);
        self.clear_waiting();
        match out {
            Some(s) if !s.trim().is_empty() => {
                let v = Zeroizing::new(s.trim_end_matches(['\r', '\n']).to_string());
                crate::redact::register_secret(&v);
                Ok(v)
            }
            _ => Err(LoaError::new(
                Code::HumanActionRequired,
                format!("{} が入力されませんでした", req.field_label),
            )),
        }
    }

    fn wait_human(
        &self,
        kind: HumanKind,
        hint: &str,
        timeout: Duration,
        done: &mut DoneCheck<'_>,
    ) -> Result<()> {
        if done()? {
            return Ok(());
        }
        self.set_waiting(kind.code(), hint);
        // Non-blocking notice window; killed when the wait ends.
        #[cfg(feature = "test-hooks")]
        let show_notice = std::env::var("LOA_TEST_NO_UI").is_err();
        #[cfg(not(feature = "test-hooks"))]
        let show_notice = true;
        let mut notice = if show_notice {
            Command::new(&self.exe)
                .args(["__ui", "notice"])
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
                .map(|mut c| {
                    if let Some(mut s) = c.stdin.take() {
                        let _ = s.write_all(
                            serde_json::json!({ "title": "loa", "message": hint })
                                .to_string()
                                .as_bytes(),
                        );
                    }
                    c
                })
        } else {
            None
        };
        let start = Instant::now();
        let result = loop {
            match done() {
                Ok(true) => break Ok(()),
                Ok(false) => {}
                Err(e) => break Err(e),
            }
            if start.elapsed() > timeout {
                break Err(LoaError::new(
                    kind.code(),
                    format!("時間内に完了しませんでした: {hint}"),
                ));
            }
            std::thread::sleep(Duration::from_secs(2));
        };
        if let Some(c) = notice.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.clear_waiting();
        result
    }

    fn progress(&self, msg: &str) {
        let _ = update_status(&self.paths, &self.id, |s| {
            s.progress = Some(msg.to_string());
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(tag: &str) -> Paths {
        let p =
            Paths::at(std::env::temp_dir().join(format!("loa-task-{tag}-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&p.root);
        p.ensure().unwrap();
        p
    }

    #[test]
    fn lifecycle_and_result() {
        let p = paths("a");
        let id = create(&p, &serde_json::json!({"cmd": "x"})).unwrap();
        assert_eq!(read_status(&p, &id).unwrap().state, TaskState::Pending);
        let env = crate::output::Envelope::new("r", "created", serde_json::json!({}));
        write_result(&p, &id, &Ok(env.clone())).unwrap();
        assert_eq!(read_status(&p, &id).unwrap().state, TaskState::Done);
        assert_eq!(read_result(&p, &id).unwrap().unwrap(), env);
    }

    #[test]
    fn dead_worker_is_detected() {
        let p = paths("b");
        let id = create(&p, &serde_json::json!({})).unwrap();
        update_status(&p, &id, |s| {
            s.state = TaskState::Running;
            s.pid = Some(1);
        })
        .unwrap();
        // nobody holds worker.lock
        let e = follow(
            &p,
            &id,
            FollowMode::UntilDone,
            Some(Duration::from_secs(5)),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(e.code, Code::WorkerLost);
    }

    #[test]
    fn exit_on_human_returns_waiting_code() {
        let p = paths("c");
        let id = create(&p, &serde_json::json!({})).unwrap();
        update_status(&p, &id, |s| {
            s.state = TaskState::Waiting;
            s.pid = Some(1);
            s.waiting = Some(Waiting {
                code: Code::LoginRequired,
                hint: "login".into(),
                since: "x".into(),
            });
        })
        .unwrap();
        let e = follow(&p, &id, FollowMode::ExitOnHuman, None, |_| {}).unwrap_err();
        assert_eq!(e.code, Code::LoginRequired);
        assert_eq!(e.task.as_deref(), Some(id.as_str()));
        assert!(e.next[0].starts_with("loa wait"));
    }

    #[test]
    fn invalid_ids_rejected() {
        let p = paths("d");
        assert!(read_status(&p, "../etc").is_err());
    }
}
