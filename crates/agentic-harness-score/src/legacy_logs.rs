use crate::{
    read_capped, score, ScoreAction, ScoreCheck, ScoreDiff, ScoreError, ScoreInput,
    ScoreMetadata, ScorePhase, ScoreResult, ScoreSource, ScoreWeights,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub fn score_legacy_logs(logs_dir: &Path) -> Result<ScoreResult, ScoreError> {
    let input = ScoreInput {
        source: ScoreSource::LegacyAgentLogs,
        run_id: logs_dir
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent_logs")
            .to_string(),
        workspace: logs_dir.display().to_string(),
        prompt: parse_legacy_prompt(&logs_dir.join("session_meta.json"))?,
        phases: parse_trace(&logs_dir.join("trace.md"))?,
        actions: parse_tool_calls(&logs_dir.join("tool_calls.json"))?,
        checks: Vec::<ScoreCheck>::new(),
        diffs: parse_diff_summary(&logs_dir.join("diff_summary.md"))?,
        plan: Vec::new(),
        changed_files: Vec::new(),
        metadata: ScoreMetadata {
            context_items: 1,
            repo_status_present: true,
            ..Default::default()
        },
    };
    Ok(score(&input, &ScoreWeights::default()))
}

fn parse_legacy_prompt(path: &Path) -> Result<String, ScoreError> {
    if !path.exists() {
        return Ok(String::new());
    }
    let text = read_capped(path)?;
    let value: Value = serde_json::from_str(&text).map_err(|source| ScoreError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(value
        .get("task_prompt")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn parse_tool_calls(path: &Path) -> Result<Vec<ScoreAction>, ScoreError> {
    let text = read_capped(path)?;
    let value: Value = serde_json::from_str(&text).map_err(|source| ScoreError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    let items = value
        .get("calls")
        .and_then(Value::as_array)
        .or_else(|| value.as_array())
        .ok_or_else(|| ScoreError::InvalidArtifact {
            path: path.to_path_buf(),
            message: "tool_calls.json must be an array or an object with calls".to_string(),
        })?;

    Ok(items
        .iter()
        .map(|item| {
            let detail = item
                .get("output_summary")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let success = item
                .get("success")
                .and_then(Value::as_bool)
                .unwrap_or_else(|| !looks_failed(&detail));
            ScoreAction {
                name: item
                    .get("tool")
                    .and_then(Value::as_str)
                    .unwrap_or("tool")
                    .to_string(),
                success,
                detail,
            }
        })
        .collect())
}

fn parse_trace(path: &Path) -> Result<Vec<ScorePhase>, ScoreError> {
    let text = read_capped(path)?;
    let mut phases = Vec::new();
    for block in text.split("\n---") {
        if !block.contains("STEP:") {
            continue;
        }
        let fields = parse_fields(block);
        let goal = fields.get("GOAL").cloned().unwrap_or_default();
        let result = fields.get("RESULT").cloned().unwrap_or_default();
        let phase = classify_phase(&goal, &result);
        phases.push(ScorePhase {
            phase,
            status: if looks_failed(&result) {
                "failed".to_string()
            } else {
                "completed".to_string()
            },
            detail: goal,
        });
    }
    Ok(phases)
}

fn parse_diff_summary(path: &Path) -> Result<Vec<ScoreDiff>, ScoreError> {
    let text = read_capped(path)?;
    let mut diffs = Vec::new();
    for block in text.split("\n---") {
        if !block.contains("STEP:") {
            continue;
        }
        let fields = parse_fields(block);
        let files = fields
            .get("FILES CHANGED")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(ToOwned::to_owned)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let lines_added = parse_u64(fields.get("LINES ADDED"));
        let lines_removed = parse_u64(fields.get("LINES REMOVED"));
        let wasted_known = files
            .iter()
            .any(|file| file.to_ascii_uppercase().contains("(DELETED)"));
        let wasted_lines = if wasted_known {
            lines_added + lines_removed
        } else {
            0
        };
        diffs.push(ScoreDiff {
            files,
            lines_added,
            lines_removed,
            wasted_lines,
            wasted_known,
        });
    }
    Ok(diffs)
}

fn parse_fields(block: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let mut current = None::<String>;
    let mut lines = Vec::<String>::new();

    for line in block.lines() {
        if let Some((name, tail)) = field_line(line) {
            if let Some(name) = current.replace(name) {
                fields.insert(name, lines.join("\n").trim().to_string());
            }
            lines.clear();
            if !tail.is_empty() {
                lines.push(tail.to_string());
            }
        } else if current.is_some() {
            lines.push(line.to_string());
        }
    }
    if let Some(name) = current {
        fields.insert(name, lines.join("\n").trim().to_string());
    }
    fields
}

fn field_line(line: &str) -> Option<(String, &str)> {
    let (name, tail) = line.split_once(':')?;
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch == ' ');
    valid.then(|| (name.trim().to_string(), tail.trim()))
}

fn classify_phase(goal: &str, result: &str) -> String {
    let text = format!("{} {}", goal, result).to_ascii_lowercase();
    if contains_any(&text, &["verify", "test", "check"]) {
        "test".to_string()
    } else if contains_any(&text, &["plan", "architecture"]) {
        "plan".to_string()
    } else if contains_any(&text, &["read", "inspect"]) {
        "inspect".to_string()
    } else if contains_any(&text, &["summary", "summarize", "final"]) {
        "summarize".to_string()
    } else {
        "edit".to_string()
    }
}

fn looks_failed(value: &str) -> bool {
    contains_any(
        &value.to_ascii_lowercase(),
        &["error", "failed", "exception", "cancelled", "traceback"],
    )
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

fn parse_u64(value: Option<&String>) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or_default()
}
