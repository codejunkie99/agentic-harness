use crate::{
    read_capped, score, RepairTelemetry, ScoreAction, ScoreCheck, ScoreDiff, ScoreError,
    ScoreInput, ScoreMetadata, ScorePhase, ScoreResult, ScoreSource, ScoreWeights,
};
use serde_json::Value;
use std::path::Path;

pub fn score_native_run(run_dir: &Path) -> Result<ScoreResult, ScoreError> {
    let run_json = run_dir.join("run.json");
    let checks_json = run_dir.join("checks.json");
    let diff_patch = run_dir.join("diff.patch");
    score_native_run_files(
        &run_json,
        checks_json.exists().then_some(checks_json.as_path()),
        diff_patch.exists().then_some(diff_patch.as_path()),
    )
}

pub fn score_native_run_files(
    run_json: &Path,
    checks_json: Option<&Path>,
    diff_patch: Option<&Path>,
) -> Result<ScoreResult, ScoreError> {
    let run_text = read_capped(run_json)?;
    let run: Value = serde_json::from_str(&run_text).map_err(|source| ScoreError::Json {
        path: run_json.to_path_buf(),
        source,
    })?;
    let checks = if let Some(path) = checks_json {
        parse_checks_file(path)?
    } else {
        parse_checks(run.get("checks").unwrap_or(&Value::Null))
    };
    let mut input = native_input(&run, checks, diff_patch)?;
    input.metadata.repo_status_present = run
        .get("gitStatusBefore")
        .and_then(Value::as_str)
        .is_some_and(|status| !status.trim().is_empty());
    let mut result = score(&input, &ScoreWeights::default());
    result.artifacts.run_json = Some(run_json.display().to_string());
    Ok(result)
}

fn native_input(
    run: &Value,
    checks: Vec<ScoreCheck>,
    diff_patch: Option<&Path>,
) -> Result<ScoreInput, ScoreError> {
    let run_id = string_field(run, "id").unwrap_or_else(|| "unknown".to_string());
    let workspace = string_field(run, "workspace").unwrap_or_else(|| ".".to_string());
    let prompt = string_field(run, "prompt").unwrap_or_default();
    let phases = parse_phases(run.get("loop").unwrap_or(&Value::Null));
    let changed_files = string_array(run.get("changedFiles").unwrap_or(&Value::Null));
    let plan = string_array(run.get("plannedSteps").unwrap_or(&Value::Null));
    let mut actions = Vec::new();

    if let Some(agent) = run.get("agent").and_then(Value::as_object) {
        actions.push(ScoreAction {
            name: "agent".to_string(),
            success: object_success(agent, "passed"),
            detail: object_status_detail(agent),
        });
    }

    for patch in array(run.get("patches").unwrap_or(&Value::Null)) {
        let success = patch
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or_else(|| value_status(patch) == Some("applied"));
        actions.push(ScoreAction {
            name: "patch".to_string(),
            success,
            detail: value_status(patch).unwrap_or("unknown").to_string(),
        });
    }

    if let Some(llm) = run.get("llm").and_then(Value::as_object) {
        actions.push(ScoreAction {
            name: "llm".to_string(),
            success: object_success(llm, "completed"),
            detail: object_status_detail(llm),
        });
    }

    for check in &checks {
        actions.push(ScoreAction {
            name: "check".to_string(),
            success: check.success,
            detail: check.command.clone(),
        });
    }

    if let Some(commit) = run.get("commit").and_then(Value::as_object) {
        actions.push(ScoreAction {
            name: "commit".to_string(),
            success: object_success(commit, "committed"),
            detail: object_status_detail(commit),
        });
    }

    if let Some(pr) = run.get("pullRequest").and_then(Value::as_object) {
        actions.push(ScoreAction {
            name: "pull-request".to_string(),
            success: object_success(pr, "created"),
            detail: object_status_detail(pr),
        });
    }

    let repair = parse_repair(run.get("repair").unwrap_or(&Value::Null));
    if let Some(repair) = &repair {
        if repair.attempted {
            actions.push(ScoreAction {
                name: "repair".to_string(),
                success: repair.success,
                detail: format!(
                    "{} failed check{} before repair",
                    repair.failed_checks_before,
                    if repair.failed_checks_before == 1 {
                        ""
                    } else {
                        "s"
                    }
                ),
            });
        }
    }

    let diffs = vec![parse_diff(diff_patch, &changed_files)?];
    let policy_blocked_edits = run
        .get("patches")
        .and_then(Value::as_array)
        .map(|patches| {
            patches
                .iter()
                .filter(|patch| {
                    patch
                        .get("blocked")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .count() as u64
        })
        .unwrap_or_default();
    let context_items = array_len(run.get("rootFiles"))
        + array_len(run.get("projectFiles"))
        + array_len(run.get("workspaceInstructions"));

    Ok(ScoreInput {
        source: ScoreSource::NativeRun,
        run_id,
        workspace,
        prompt,
        phases,
        actions,
        checks,
        diffs,
        plan,
        changed_files,
        metadata: ScoreMetadata {
            context_items,
            policy_blocked_edits,
            repair,
            ..Default::default()
        },
    })
}

fn parse_checks_file(path: &Path) -> Result<Vec<ScoreCheck>, ScoreError> {
    let text = read_capped(path)?;
    let value: Value = serde_json::from_str(&text).map_err(|source| ScoreError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(parse_checks(&value))
}

fn parse_checks(value: &Value) -> Vec<ScoreCheck> {
    array(value)
        .into_iter()
        .map(|check| ScoreCheck {
            command: string_field(check, "command").unwrap_or_else(|| "unknown".to_string()),
            success: check
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| value_status(check) == Some("passed")),
            exit_code: check.get("exitCode").and_then(Value::as_i64),
        })
        .collect()
}

fn parse_phases(value: &Value) -> Vec<ScorePhase> {
    array(value)
        .into_iter()
        .map(|phase| ScorePhase {
            phase: string_field(phase, "phase").unwrap_or_else(|| "unknown".to_string()),
            status: string_field(phase, "status").unwrap_or_else(|| "unknown".to_string()),
            detail: string_field(phase, "detail").unwrap_or_default(),
        })
        .collect()
}

fn parse_repair(value: &Value) -> Option<RepairTelemetry> {
    let object = value.as_object()?;
    Some(RepairTelemetry {
        attempted: object
            .get("attempted")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        success: object
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        failed_checks_before: object
            .get("failedChecksBefore")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
        checks_passed_after: object
            .get("checksPassedAfter")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        patches_applied: object
            .get("patchesApplied")
            .and_then(Value::as_u64)
            .unwrap_or_default(),
    })
}

fn parse_diff(path: Option<&Path>, changed_files: &[String]) -> Result<ScoreDiff, ScoreError> {
    let Some(path) = path else {
        return Ok(ScoreDiff {
            files: changed_files.to_vec(),
            lines_added: 0,
            lines_removed: 0,
            wasted_lines: 0,
            wasted_known: false,
        });
    };
    let text = read_capped(path)?;
    let mut lines_added = 0;
    let mut lines_removed = 0;
    let mut files = changed_files.to_vec();

    for line in text.lines() {
        if line.starts_with("diff --git ") {
            if let Some(file) = line
                .split_whitespace()
                .nth(3)
                .and_then(|part| part.strip_prefix("b/"))
            {
                if !files.iter().any(|existing| existing == file) {
                    files.push(file.to_string());
                }
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            lines_added += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            lines_removed += 1;
        }
    }

    Ok(ScoreDiff {
        files,
        lines_added,
        lines_removed,
        wasted_lines: 0,
        wasted_known: false,
    })
}

fn object_success(object: &serde_json::Map<String, Value>, success_status: &str) -> bool {
    object
        .get("success")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            object
                .get("status")
                .and_then(Value::as_str)
                .is_some_and(|status| status == success_status || status == "passed")
        })
}

fn object_status_detail(object: &serde_json::Map<String, Value>) -> String {
    object
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string()
}

fn value_status(value: &Value) -> Option<&str> {
    value.get("status").and_then(Value::as_str)
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn string_array(value: &Value) -> Vec<String> {
    array(value)
        .into_iter()
        .filter_map(Value::as_str)
        .map(ToOwned::to_owned)
        .collect()
}

fn array(value: &Value) -> Vec<&Value> {
    value
        .as_array()
        .map(|items| items.iter().collect())
        .unwrap_or_default()
}

fn array_len(value: Option<&Value>) -> u64 {
    value
        .and_then(Value::as_array)
        .map(|items| items.len() as u64)
        .unwrap_or_default()
}
