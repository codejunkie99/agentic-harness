use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ScoreSource {
    NativeRun,
    LegacyAgentLogs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreInput {
    pub source: ScoreSource,
    pub run_id: String,
    pub workspace: String,
    pub prompt: String,
    pub phases: Vec<ScorePhase>,
    pub actions: Vec<ScoreAction>,
    pub checks: Vec<ScoreCheck>,
    pub diffs: Vec<ScoreDiff>,
    pub plan: Vec<String>,
    pub changed_files: Vec<String>,
    pub metadata: ScoreMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScorePhase {
    pub phase: String,
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreAction {
    pub name: String,
    pub success: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreCheck {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreDiff {
    pub files: Vec<String>,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub wasted_lines: u64,
    pub wasted_known: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairTelemetry {
    pub attempted: bool,
    pub success: bool,
    pub failed_checks_before: u64,
    pub checks_passed_after: bool,
    pub patches_applied: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreMetadata {
    pub model: Option<String>,
    pub date: Option<String>,
    pub context_items: u64,
    pub repo_status_present: bool,
    pub policy_blocked_edits: u64,
    pub repair: Option<RepairTelemetry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreMetricBreakdown {
    pub completion: f64,
    pub efficiency: f64,
    pub tool_success: f64,
    pub recovery: f64,
    pub diff_quality: f64,
    pub planning_quality: f64,
    pub composite: f64,
    pub completion_detail: String,
    pub efficiency_detail: String,
    pub tool_success_detail: String,
    pub recovery_detail: String,
    pub diff_quality_detail: String,
    pub planning_quality_detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreStats {
    pub total_phases: u64,
    pub failed_phases: u64,
    pub total_actions: u64,
    pub failed_actions: u64,
    pub total_checks: u64,
    pub failed_checks: u64,
    pub lines_added: u64,
    pub lines_removed: u64,
    pub wasted_lines: u64,
    pub changed_files: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreArtifacts {
    pub run_json: Option<String>,
    pub markdown: Option<String>,
    pub json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreResult {
    pub run_id: String,
    pub workspace: String,
    pub prompt: String,
    pub score: f64,
    pub grade: String,
    pub metrics: ScoreMetricBreakdown,
    pub stats: ScoreStats,
    pub artifacts: ScoreArtifacts,
    pub warnings: Vec<String>,
}
