//! Failure reports (spec 7-5, v1.1 change 6). `issue.md` is safe to paste into an Issue:
//! step, URL without query/fragment and a redacted snapshot. The screenshot stays local.

use crate::browser::{Browser, Snapshot};
use crate::error::LoaError;
use crate::scenario::model::Scenario;
use serde_json::json;
use std::path::{Path, PathBuf};

#[allow(clippy::too_many_arguments)]
pub fn write_failure(
    reports_dir: &Path,
    sc: &Scenario,
    step_no: &str,
    step_kind: &str,
    url: &str,
    err: &LoaError,
    snapshot: Option<&Snapshot>,
    browser: &mut dyn Browser,
) -> Option<PathBuf> {
    let dir = reports_dir.join(format!("{}-{}", crate::util::now_compact(), sc.id));
    crate::paths::create_private_dir(&dir).ok()?;
    let safe_url = crate::redact::redact_url(url);
    let snap_text = snapshot
        .map(|s| crate::redact::redact(&s.to_text()))
        .unwrap_or_default();
    let message = crate::redact::redact(&err.message);
    let report = json!({
        "loa_version": crate::util::LOA_VERSION,
        "scenario": sc.id,
        "scenario_version": sc.version,
        "scenario_status": sc.status,
        "operation": sc.operation,
        "step": step_no,
        "step_kind": step_kind,
        "url": safe_url,
        "code": err.code,
        "message": message,
        "os": std::env::consts::OS,
        "at": crate::util::now_rfc3339(),
    });
    let _ = crate::paths::write_atomic(
        &dir.join("report.json"),
        serde_json::to_string_pretty(&report)
            .unwrap_or_default()
            .as_bytes(),
    );
    let issue = format!(
        "## シナリオの失敗\n\n| 項目 | 値 |\n|---|---|\n| loa | {} |\n| シナリオ | {} (v{}, {:?}) |\n| ステップ | {} ({}) |\n| URL | {} |\n| コード | {} |\n| OS | {} |\n\n### メッセージ\n\n{}\n\n### 画面の構造（伏せ字済み）\n\n```\n{}\n```\n",
        crate::util::LOA_VERSION,
        sc.id,
        sc.version,
        sc.status,
        step_no,
        step_kind,
        safe_url,
        err.code,
        std::env::consts::OS,
        message,
        truncate_lines(&snap_text, 400),
    );
    let _ = crate::paths::write_atomic(&dir.join("issue.md"), issue.as_bytes());
    // Local only: screenshots can show secrets and are not part of issue.md.
    let _ = browser.screenshot(&dir.join("screenshot-local-only.png"));
    Some(dir)
}

fn truncate_lines(s: &str, max: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}\n… ({} 行省略)",
            lines[..max].join("\n"),
            lines.len() - max
        )
    }
}
