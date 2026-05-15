use agentic_harness_score::{render_markdown, score_legacy_logs, score_native_run};
use std::fs;

#[test]
fn native_run_scores_from_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let run = temp.path();
    fs::write(
        run.join("run.json"),
        r#"{
  "workspace": "/repo",
  "id": "code",
  "prompt": "Fix tests",
  "loop": [
    {"phase":"inspect","status":"completed","detail":"workspace inspected"},
    {"phase":"plan","status":"completed","detail":"4 planned steps"},
    {"phase":"edit","status":"completed","detail":"1 changed file"},
    {"phase":"test","status":"passed","detail":"1/1 check passed"},
    {"phase":"summarize","status":"completed","detail":"summary written"}
  ],
  "plannedSteps": ["Inspect repository", "Run checks"],
  "rootFiles": ["Cargo.toml"],
  "projectFiles": ["src/main.rs"],
  "workspaceInstructions": [],
  "changedFiles": ["src/main.rs"],
  "agent": {"status":"passed","success":true},
  "checks": [{"command":"cargo test","status":"passed","success":true,"exitCode":0}]
}"#,
    )
    .unwrap();
    fs::write(
        run.join("checks.json"),
        r#"[{"command":"cargo test","status":"passed","success":true,"exitCode":0}]"#,
    )
    .unwrap();
    fs::write(
        run.join("diff.patch"),
        "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n",
    )
    .unwrap();

    let result = score_native_run(run).unwrap();
    assert!(result.score >= 0.85, "{result:#?}");
    assert_eq!(result.grade, "A");
    assert_eq!(result.stats.total_checks, 1);
    assert!(render_markdown(&result).contains("Harness score"));
}

#[test]
fn native_run_penalizes_failed_checks() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("run.json"),
        r#"{
  "workspace": "/repo",
  "id": "code",
  "loop": [
    {"phase":"inspect","status":"completed","detail":""},
    {"phase":"plan","status":"completed","detail":""},
    {"phase":"edit","status":"completed","detail":""},
    {"phase":"test","status":"failed","detail":"0/1 check passed"},
    {"phase":"summarize","status":"completed","detail":""}
  ],
  "plannedSteps": ["Inspect repository"],
  "agent": {"status":"passed","success":true},
  "checks": [{"command":"cargo test","status":"failed","success":false,"exitCode":101}]
}"#,
    )
    .unwrap();
    fs::write(temp.path().join("checks.json"), r#"[{"command":"cargo test","success":false}]"#)
        .unwrap();
    fs::write(temp.path().join("diff.patch"), "").unwrap();

    let result = score_native_run(temp.path()).unwrap();
    assert!(result.score < 0.85, "{result:#?}");
    assert_eq!(result.stats.failed_checks, 1);
}

#[test]
fn legacy_logs_are_supported() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("trace.md"),
        "# Agent Trace Log\n\n---\n\nSTEP: 1\n\nGOAL: Inspect files\n\nRESULT: ok\n\n---\n\nSTEP: 2\n\nGOAL: Verify tests\n\nRESULT: ok\n\n---\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("tool_calls.json"),
        r#"[{"step":1,"tool":"Read","output_summary":"ok","success":true},{"step":2,"tool":"Bash","output_summary":"ok","success":true}]"#,
    )
    .unwrap();
    fs::write(
        temp.path().join("diff_summary.md"),
        "# Diff Summary\n\n---\n\nSTEP: 1\nFILES CHANGED: src/main.rs\nLINES ADDED: 2\nLINES REMOVED: 1\nREASON FOR CHANGE: focused edit\n\n---\n",
    )
    .unwrap();
    fs::write(
        temp.path().join("session_meta.json"),
        r#"{"task_prompt":"Improve implementation"}"#,
    )
    .unwrap();

    let result = score_legacy_logs(temp.path()).unwrap();
    assert!(result.score > 0.5, "{result:#?}");
    assert_eq!(result.prompt, "Improve implementation");
}
