use crate::ScoreResult;

pub fn render_markdown(result: &ScoreResult) -> String {
    let metrics = &result.metrics;
    let mut out = String::new();
    out.push_str("# Agentic Harness Score\n\n");
    out.push_str(&format!("Run: `{}`\n\n", result.run_id));
    out.push_str(&format!("Workspace: `{}`\n\n", result.workspace));
    if !result.prompt.trim().is_empty() {
        out.push_str(&format!("Prompt: {}\n\n", result.prompt));
    }
    out.push_str(&format!(
        "Harness score: **{:.3} / 1.000** ({})\n\n",
        result.score, result.grade
    ));
    out.push_str("## Formula\n\n");
    out.push_str("```text\n");
    out.push_str("score = 0.30 * completion\n");
    out.push_str("      + 0.18 * efficiency\n");
    out.push_str("      + 0.15 * tool_success\n");
    out.push_str("      + 0.15 * recovery\n");
    out.push_str("      + 0.12 * diff_quality\n");
    out.push_str("      + 0.10 * planning_quality\n");
    out.push_str("```\n\n");
    out.push_str("## Breakdown\n\n");
    out.push_str("| Metric | Value | Detail |\n");
    out.push_str("|---|---:|---|\n");
    out.push_str(&metric_row(
        "Completion",
        metrics.completion,
        &metrics.completion_detail,
    ));
    out.push_str(&metric_row(
        "Efficiency",
        metrics.efficiency,
        &metrics.efficiency_detail,
    ));
    out.push_str(&metric_row(
        "Tool success",
        metrics.tool_success,
        &metrics.tool_success_detail,
    ));
    out.push_str(&metric_row("Recovery", metrics.recovery, &metrics.recovery_detail));
    out.push_str(&metric_row(
        "Diff quality",
        metrics.diff_quality,
        &metrics.diff_quality_detail,
    ));
    out.push_str(&metric_row(
        "Planning quality",
        metrics.planning_quality,
        &metrics.planning_quality_detail,
    ));
    out.push_str(&format!("| **Composite** | **{:.3}** | |\n", metrics.composite));

    out.push_str("\n## Run Stats\n\n");
    out.push_str("| Stat | Value |\n");
    out.push_str("|---|---:|\n");
    out.push_str(&format!("| Total phases | {} |\n", result.stats.total_phases));
    out.push_str(&format!("| Failed phases | {} |\n", result.stats.failed_phases));
    out.push_str(&format!("| Total actions | {} |\n", result.stats.total_actions));
    out.push_str(&format!("| Failed actions | {} |\n", result.stats.failed_actions));
    out.push_str(&format!("| Total checks | {} |\n", result.stats.total_checks));
    out.push_str(&format!("| Failed checks | {} |\n", result.stats.failed_checks));
    out.push_str(&format!("| Lines added | {} |\n", result.stats.lines_added));
    out.push_str(&format!("| Lines removed | {} |\n", result.stats.lines_removed));
    out.push_str(&format!("| Wasted lines | {} |\n", result.stats.wasted_lines));
    out.push_str(&format!("| Changed files | {} |\n", result.stats.changed_files));

    if !result.warnings.is_empty() {
        out.push_str("\n## Warnings\n\n");
        for warning in &result.warnings {
            out.push_str(&format!("- {warning}\n"));
        }
    }
    out
}

fn metric_row(label: &str, value: f64, detail: &str) -> String {
    format!("| {label} | {value:.3} | {} |\n", escape_table(detail))
}

fn escape_table(value: &str) -> String {
    value.replace('|', "\\|")
}
