use crate::{
    RepairTelemetry, ScoreInput, ScoreMetricBreakdown, ScoreResult, ScoreSource, ScoreStats,
    ScoreWeights,
};

const REQUIRED_PHASES: &[&str] = &["inspect", "plan", "edit", "test", "summarize"];

pub fn score(input: &ScoreInput, weights: &ScoreWeights) -> ScoreResult {
    let mut warnings = Vec::new();
    let (completion, completion_detail) = completion(input);
    let (efficiency, efficiency_detail) = efficiency(input);
    let (tool_success, tool_success_detail) = tool_success(input);
    let (recovery, recovery_detail) = recovery(input);
    let (diff_quality, diff_quality_detail) = diff_quality(input, &mut warnings);
    let (planning_quality, planning_quality_detail) = planning_quality(input, &mut warnings);

    let composite = weights.completion * completion
        + weights.efficiency * efficiency
        + weights.tool_success * tool_success
        + weights.recovery * recovery
        + weights.diff_quality * diff_quality
        + weights.planning_quality * planning_quality;

    ScoreResult {
        run_id: input.run_id.clone(),
        workspace: input.workspace.clone(),
        prompt: input.prompt.clone(),
        score: round3(composite),
        grade: grade(composite).to_string(),
        metrics: ScoreMetricBreakdown {
            completion: round3(completion),
            efficiency: round3(efficiency),
            tool_success: round3(tool_success),
            recovery: round3(recovery),
            diff_quality: round3(diff_quality),
            planning_quality: round3(planning_quality),
            composite: round3(composite),
            completion_detail,
            efficiency_detail,
            tool_success_detail,
            recovery_detail,
            diff_quality_detail,
            planning_quality_detail,
        },
        stats: stats(input),
        artifacts: Default::default(),
        warnings,
    }
}

fn completion(input: &ScoreInput) -> (f64, String) {
    let required = input
        .phases
        .iter()
        .filter(|phase| REQUIRED_PHASES.contains(&phase.phase.as_str()))
        .collect::<Vec<_>>();
    if required.is_empty() {
        if input.actions.is_empty() {
            return (0.0, "no phases or actions recorded".to_string());
        }
        let successful = input.actions.iter().filter(|action| action.success).count();
        return (
            successful as f64 / input.actions.len() as f64,
            format!("{successful}/{} actions succeeded", input.actions.len()),
        );
    }

    let successful = required
        .iter()
        .filter(|phase| phase_success_counts(&phase.phase, &phase.status))
        .count();
    (
        successful as f64 / required.len() as f64,
        format!("{successful}/{} required phases succeeded", required.len()),
    )
}

fn efficiency(input: &ScoreInput) -> (f64, String) {
    let active = input
        .phases
        .iter()
        .filter(|phase| !optional_skipped_phase(&phase.phase, &phase.status))
        .collect::<Vec<_>>();
    if active.is_empty() {
        return (1.0, "no active phases recorded".to_string());
    }
    let completed = active
        .iter()
        .filter(|phase| !is_failed_status(&phase.status))
        .count();
    let failed_actions = input.actions.iter().filter(|action| !action.success).count();
    let repair_penalty = input
        .metadata
        .repair
        .as_ref()
        .is_some_and(|repair| repair.attempted) as u8 as f64
        * 0.05;
    let action_penalty = (failed_actions as f64 * 0.04).min(0.25);
    let raw = completed as f64 / active.len() as f64;
    let adjusted = (raw - repair_penalty - action_penalty).clamp(0.0, 1.0);
    (
        adjusted,
        format!(
            "{completed}/{} active phases completed, {failed_actions} failed action{}",
            active.len(),
            plural(failed_actions)
        ),
    )
}

fn tool_success(input: &ScoreInput) -> (f64, String) {
    if input.actions.is_empty() {
        return (1.0, "no tool or gate actions recorded".to_string());
    }
    let successful = input.actions.iter().filter(|action| action.success).count();
    (
        successful as f64 / input.actions.len() as f64,
        format!("{successful}/{} actions succeeded", input.actions.len()),
    )
}

fn recovery(input: &ScoreInput) -> (f64, String) {
    let failed_positions = input
        .actions
        .iter()
        .enumerate()
        .filter(|(_, action)| !action.success)
        .map(|(index, action)| (index, action.name.as_str()))
        .collect::<Vec<_>>();
    if failed_positions.is_empty() {
        return (1.0, "no failures to recover from".to_string());
    }

    let repair = input.metadata.repair.as_ref();
    let recovered = failed_positions
        .iter()
        .filter(|(index, name)| {
            later_matching_success(input, *index, name)
                || repair_recovers_action(repair, name)
        })
        .count();
    (
        recovered as f64 / failed_positions.len() as f64,
        format!("{recovered}/{} failed actions recovered", failed_positions.len()),
    )
}

fn diff_quality(input: &ScoreInput, warnings: &mut Vec<String>) -> (f64, String) {
    let lines_added = input.diffs.iter().map(|diff| diff.lines_added).sum::<u64>();
    let lines_removed = input
        .diffs
        .iter()
        .map(|diff| diff.lines_removed)
        .sum::<u64>();
    let total = lines_added + lines_removed;
    if total == 0 {
        return (1.0, "no diff lines recorded".to_string());
    }

    let wasted = input.diffs.iter().map(|diff| diff.wasted_lines).sum::<u64>();
    if input.source == ScoreSource::NativeRun && input.diffs.iter().all(|diff| !diff.wasted_known)
    {
        warnings.push(
            "diff quality is approximate because native runs do not yet record intermediate wasted diffs"
                .to_string(),
        );
    }
    let failed_checks = input.checks.iter().filter(|check| !check.success).count();
    let wasted_ratio = (wasted as f64 / total as f64).clamp(0.0, 1.0);
    let mut value = 1.0 - wasted_ratio;
    if failed_checks > 0 {
        value *= 0.65;
    }
    (
        value.clamp(0.0, 1.0),
        format!(
            "{wasted}/{total} wasted lines, {failed_checks} failed check{}",
            plural(failed_checks)
        ),
    )
}

fn planning_quality(input: &ScoreInput, warnings: &mut Vec<String>) -> (f64, String) {
    if input.source == ScoreSource::NativeRun {
        warnings.push(
            "planning quality is approximate until read/write ordering telemetry is recorded"
                .to_string(),
        );
    }

    let has_plan = !input.plan.is_empty();
    let has_context = input.metadata.context_items > 0;
    let has_repo_status = input.metadata.repo_status_present;
    let mentions_checks = input
        .plan
        .iter()
        .any(|step| contains_any_ci(step, &["check", "test", "verify"]));
    let no_policy_blocks = input.metadata.policy_blocked_edits == 0;
    let parts = [
        has_plan,
        has_context,
        has_repo_status,
        mentions_checks,
        no_policy_blocks,
    ];
    let passed = parts.iter().filter(|value| **value).count();
    (
        passed as f64 / parts.len() as f64,
        format!("{passed}/{} planning signals present", parts.len()),
    )
}

fn stats(input: &ScoreInput) -> ScoreStats {
    ScoreStats {
        total_phases: input.phases.len() as u64,
        failed_phases: input
            .phases
            .iter()
            .filter(|phase| is_failed_status(&phase.status))
            .count() as u64,
        total_actions: input.actions.len() as u64,
        failed_actions: input.actions.iter().filter(|action| !action.success).count() as u64,
        total_checks: input.checks.len() as u64,
        failed_checks: input.checks.iter().filter(|check| !check.success).count() as u64,
        lines_added: input.diffs.iter().map(|diff| diff.lines_added).sum(),
        lines_removed: input.diffs.iter().map(|diff| diff.lines_removed).sum(),
        wasted_lines: input.diffs.iter().map(|diff| diff.wasted_lines).sum(),
        changed_files: input.changed_files.len() as u64,
    }
}

fn phase_success_counts(phase: &str, status: &str) -> bool {
    if phase == "test" && status == "skipped" {
        return true;
    }
    !is_failed_status(status)
}

fn optional_skipped_phase(phase: &str, status: &str) -> bool {
    matches!(phase, "commit" | "pull-request") && status == "skipped"
}

fn is_failed_status(status: &str) -> bool {
    matches!(status, "failed" | "blocked" | "error")
}

fn later_matching_success(input: &ScoreInput, index: usize, name: &str) -> bool {
    input
        .actions
        .iter()
        .skip(index + 1)
        .any(|action| action.success && action.name == name)
}

fn repair_recovers_action(repair: Option<&RepairTelemetry>, name: &str) -> bool {
    repair.is_some_and(|repair| {
        repair.success
            && matches!(
                name,
                "check" | "patch" | "agent" | "repair" | "llm" | "coding-agent"
            )
    })
}

fn contains_any_ci(value: &str, needles: &[&str]) -> bool {
    let lower = value.to_ascii_lowercase();
    needles.iter().any(|needle| lower.contains(needle))
}

fn grade(score: f64) -> &'static str {
    if score >= 0.90 {
        "A"
    } else if score >= 0.80 {
        "B+"
    } else if score >= 0.70 {
        "B"
    } else if score >= 0.60 {
        "C"
    } else {
        "D"
    }
}

fn round3(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}
