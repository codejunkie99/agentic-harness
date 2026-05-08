//! Agentic Harness runtime.
//!
//! This crate is the Rust-native foundation for Agentic Harness agents. A Rust agent is a
//! normal Rust binary that registers handlers with [`AgentApp`], then either
//! invokes them directly, serves them over the Agentic Harness HTTP route shape, or exposes
//! a small embedded CLI via [`run_cli`].

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
#[cfg(feature = "native")]
use std::collections::HashMap;
use std::collections::{BTreeMap, BTreeSet};
#[cfg(feature = "native")]
use std::fs;
use std::hash::{Hash, Hasher};
#[cfg(feature = "native")]
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(feature = "native")]
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
#[cfg(feature = "native")]
use std::process::{Command, Output, Stdio};
#[cfg(feature = "native")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(feature = "native")]
use std::thread;
#[cfg(feature = "native")]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;

type AgentResult = Result<Value, AgenticHarnessError>;
type Handler = dyn Fn(AgentContext) -> AgentResult + Send + Sync + 'static;
type SharedSessionStore = Arc<Mutex<BTreeMap<String, SessionData>>>;
type SharedPersistedSessionStore = Arc<dyn SessionStore>;
type SharedRuntimeEvents = Arc<dyn Fn(RuntimeEvent) + Send + Sync + 'static>;
type SharedModelClient = Arc<dyn ModelClient>;
type SharedSessionEnv = Arc<dyn SessionEnv>;
type CommandHandler =
    dyn Fn(&[String]) -> Result<ShellOutput, AgenticHarnessError> + Send + Sync + 'static;
type ToolHandler = dyn Fn(Value) -> Result<String, AgenticHarnessError> + Send + Sync + 'static;

const BUILTIN_TOOL_NAMES: &[&str] = &["read", "write", "edit", "bash", "grep", "glob", "task"];
const MAX_TOOL_CALL_ROUNDS: usize = 8;
const CLOUDFLARE_COMPATIBILITY_DATE: &str = "2026-05-08";

/// Common SDK imports for authoring native Rust agents.
pub mod prelude {
    pub use crate::{
        mcp_tools_from_client, AgentApp, AgentContext, AgentDefinition, AgentRuntimeConfig,
        AgenticHarnessError, CommandDef, CompactionOptions, CompactionSettings, FileStat,
        MemorySessionEnv, ModelClient, ModelMessage, ModelRequest, PromptOptions, PromptResponse,
        ProviderSettings, ProvidersConfig, ReadOptions, ReadOutput, Role, RuntimeEvent,
        SandboxConnector, SandboxProvider, Session, SessionEnv, ShellOptions, ShellOutput, Skill,
        ToolCall, ToolDef, ToolSpec, VirtualSessionEnv,
    };
    #[cfg(feature = "native")]
    pub use crate::{
        run_cli, run_cli_with_args, RepoInspection, SoftwareCheck, SoftwareCheckResult,
        SoftwareCommit, SoftwareInstruction, SoftwarePatch, SoftwarePlan, SoftwarePullRequest,
        SoftwareSummary, SoftwareWorkspace,
    };
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeEvent {
    event: String,
    data: Value,
}

impl RuntimeEvent {
    pub fn new(event: impl Into<String>, data: Value) -> Self {
        Self {
            event: event.into(),
            data,
        }
    }

    pub fn event(&self) -> &str {
        &self.event
    }

    pub fn data(&self) -> &Value {
        &self.data
    }

    pub fn to_sse_frame(&self) -> Result<String, AgenticHarnessError> {
        let mut frame = String::new();
        frame.push_str("event: ");
        frame.push_str(&self.event);
        frame.push('\n');
        frame.push_str("data: ");
        frame.push_str(&serde_json::to_string(&self.data)?);
        frame.push_str("\n\n");
        Ok(frame)
    }
}

/// Runtime-agnostic storage for persisted session data.
pub trait SessionStore: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<SessionData>, AgenticHarnessError>;
    fn save(&self, key: &str, data: &SessionData) -> Result<(), AgenticHarnessError>;
    fn delete(&self, key: &str) -> Result<(), AgenticHarnessError>;
}

/// Error type used across the native Agentic Harness runtime.
#[derive(Debug, Error)]
pub enum AgenticHarnessError {
    #[error("Agent {name:?} was not found.")]
    AgentNotFound {
        name: String,
        available: Vec<String>,
    },
    #[error("Route not found: {method} {path}")]
    RouteNotFound { method: String, path: String },
    #[error("Method {method} is not allowed for this route.")]
    MethodNotAllowed { method: String },
    #[error("Invalid JSON body: {message}")]
    InvalidJson { message: String },
    #[error("Invalid regex pattern: {message}")]
    InvalidRegex { message: String },
    #[error("Invalid payload: {message}")]
    InvalidPayload { message: String },
    #[error("Workspace context {kind} {name:?} was not found.")]
    ContextNotFound { kind: &'static str, name: String },
    #[error("Command {name:?} was not found.")]
    CommandNotFound {
        name: String,
        available: Vec<String>,
    },
    #[error("Tool {name:?} was not found.")]
    ToolNotFound {
        name: String,
        available: Vec<String>,
    },
    #[error("Tool {name:?} conflicts with a built-in tool.")]
    ToolNameConflict { name: String },
    #[error("Duplicate tool name {name:?}.")]
    DuplicateTool { name: String },
    #[error("Invalid model config {model:?}: {message}")]
    InvalidModel { model: String, message: String },
    #[error("Invalid deployment target {target:?}: {message}")]
    InvalidDeployment { target: String, message: String },
    #[error("Model {model:?} was not found.")]
    ModelNotFound {
        model: String,
        available: Vec<String>,
    },
    #[error("Provider request failed: {message}")]
    Provider { message: String },
    #[error("MCP request failed: {message}")]
    Mcp { message: String },
    #[cfg(feature = "native")]
    #[error("HTTP client error: {source}")]
    Http {
        #[from]
        source: reqwest::Error,
    },
    #[error("Result extraction failed: {message}")]
    ResultExtraction { message: String },
    #[error("Agent handler failed: {message}")]
    Handler { message: String },
    #[error("I/O error: {source}")]
    Io {
        #[from]
        source: std::io::Error,
    },
    #[error("JSON error: {source}")]
    Json {
        #[from]
        source: serde_json::Error,
    },
}

impl AgenticHarnessError {
    /// Stable machine-readable error type used by HTTP envelopes.
    pub fn error_type(&self) -> &'static str {
        match self {
            Self::AgentNotFound { .. } => "agent_not_found",
            Self::RouteNotFound { .. } => "route_not_found",
            Self::MethodNotAllowed { .. } => "method_not_allowed",
            Self::InvalidJson { .. } => "invalid_json",
            Self::InvalidRegex { .. } => "invalid_regex",
            Self::InvalidPayload { .. } => "invalid_payload",
            Self::ContextNotFound { kind, .. } => match *kind {
                "role" => "role_not_found",
                "skill" => "skill_not_found",
                _ => "context_not_found",
            },
            Self::CommandNotFound { .. } => "command_not_found",
            Self::ToolNotFound { .. } => "tool_not_found",
            Self::ToolNameConflict { .. } => "tool_name_conflict",
            Self::DuplicateTool { .. } => "duplicate_tool",
            Self::InvalidModel { .. } => "invalid_model",
            Self::InvalidDeployment { .. } => "invalid_deployment",
            Self::ModelNotFound { .. } => "model_not_found",
            Self::Provider { .. } => "provider_error",
            Self::Mcp { .. } => "mcp_error",
            #[cfg(feature = "native")]
            Self::Http { .. } => "http_error",
            Self::ResultExtraction { .. } => "result_extraction_error",
            Self::Handler { .. } => "handler_error",
            Self::Io { .. } => "io_error",
            Self::Json { .. } => "json_error",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            Self::AgentNotFound { .. } | Self::RouteNotFound { .. } => 404,
            Self::MethodNotAllowed { .. } => 405,
            Self::InvalidJson { .. } | Self::InvalidRegex { .. } | Self::InvalidPayload { .. } => {
                400
            }
            Self::ContextNotFound { .. }
            | Self::CommandNotFound { .. }
            | Self::ToolNotFound { .. }
            | Self::ToolNameConflict { .. }
            | Self::DuplicateTool { .. }
            | Self::InvalidModel { .. }
            | Self::InvalidDeployment { .. } => 400,
            Self::ModelNotFound { .. } | Self::Provider { .. } | Self::Mcp { .. } => 500,
            #[cfg(feature = "native")]
            Self::Http { .. } => 500,
            Self::ResultExtraction { .. }
            | Self::Handler { .. }
            | Self::Io { .. }
            | Self::Json { .. } => 500,
        }
    }

    fn message(&self) -> String {
        match self {
            Self::AgentNotFound { name, .. } => format!("Agent \"{name}\" was not found."),
            Self::RouteNotFound { method, path } => format!("No route for {method} {path}."),
            Self::MethodNotAllowed { method } => {
                format!("Method \"{method}\" is not allowed for this route.")
            }
            Self::InvalidJson { message } => format!("Invalid JSON body: {message}"),
            Self::InvalidRegex { message } => format!("Invalid regex pattern: {message}"),
            Self::InvalidPayload { message } => format!("Invalid payload: {message}"),
            Self::ContextNotFound { kind, name } => {
                format!("Workspace {kind} \"{name}\" was not found.")
            }
            Self::CommandNotFound { name, .. } => {
                format!("Command \"{name}\" was not registered.")
            }
            Self::ToolNotFound { name, .. } => format!("Tool \"{name}\" was not registered."),
            Self::ToolNameConflict { name } => {
                format!("Tool \"{name}\" conflicts with a built-in tool.")
            }
            Self::DuplicateTool { name } => format!("Duplicate tool name \"{name}\"."),
            Self::InvalidModel { model, message } => {
                format!("Invalid model config \"{model}\": {message}")
            }
            Self::InvalidDeployment { target, message } => {
                format!("Invalid deployment target \"{target}\": {message}")
            }
            Self::ModelNotFound { model, .. } => format!("Model \"{model}\" was not registered."),
            Self::Provider { message } => message.clone(),
            Self::Mcp { message } => message.clone(),
            #[cfg(feature = "native")]
            Self::Http { source } => source.to_string(),
            Self::ResultExtraction { message } => message.clone(),
            Self::Handler { message } => message.clone(),
            Self::Io { source } => source.to_string(),
            Self::Json { source } => source.to_string(),
        }
    }

    fn details(&self) -> Option<String> {
        match self {
            Self::AgentNotFound { available, .. } => Some(format!(
                "Available agents: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )),
            Self::RouteNotFound { .. } => Some("Expected route: /agents/<name>/<id>".to_string()),
            Self::MethodNotAllowed { .. } => Some("Use POST for agent invocations.".to_string()),
            Self::ContextNotFound { kind, name } => Some(format!(
                "Known {kind}s are loaded from the workspace at startup; missing: {name}."
            )),
            Self::ModelNotFound { available, .. } => Some(format!(
                "Available models: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )),
            Self::CommandNotFound { available, .. } => Some(format!(
                "Available commands: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )),
            Self::ToolNotFound { available, .. } => Some(format!(
                "Available tools: {}",
                if available.is_empty() {
                    "(none)".to_string()
                } else {
                    available.join(", ")
                }
            )),
            Self::ToolNameConflict { .. } => {
                Some(format!("Built-in tools: {}", BUILTIN_TOOL_NAMES.join(", ")))
            }
            _ => None,
        }
    }

    fn envelope(&self) -> Value {
        let mut error = serde_json::Map::new();
        error.insert("type".to_string(), json!(self.error_type()));
        error.insert("message".to_string(), json!(self.message()));
        if let Some(details) = self.details() {
            error.insert("details".to_string(), json!(details));
        }
        json!({ "error": Value::Object(error) })
    }
}

/// Role markdown loaded from `<workspace>/.agentic-harness/roles` or `<workspace>/roles`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Role {
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model: Option<String>,
}

/// Skill markdown loaded from `<workspace>/.agents/skills/<name>/SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub instructions: String,
}

/// Shell command result from [`AgentContext::shell`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Options for scoped shell execution.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShellOptions {
    pub cwd: Option<PathBuf>,
    pub env: BTreeMap<String, String>,
    pub timeout: Option<Duration>,
}

impl ShellOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

/// Filesystem metadata returned by [`AgentContext::stat`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStat {
    pub is_file: bool,
    pub is_directory: bool,
    pub is_symbolic_link: bool,
    pub size: u64,
    pub modified_unix_ms: Option<u64>,
}

/// Options for bounded UTF-8 file reads.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReadOptions {
    pub offset: usize,
    pub max_bytes: Option<usize>,
}

impl ReadOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    pub fn max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = Some(max_bytes);
        self
    }
}

/// Result from [`AgentContext::read_with_options`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReadOutput {
    pub content: String,
    pub start_byte: usize,
    pub bytes_read: usize,
    pub total_bytes: usize,
    pub truncated: bool,
}

/// Repository instruction file loaded by the software lifecycle SDK.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwareInstruction {
    pub path: String,
    pub content: String,
}

/// Snapshot of repository context before a software-agent run edits files.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoInspection {
    pub root: PathBuf,
    pub root_files: Vec<String>,
    pub project_files: Vec<String>,
    pub instructions: Vec<SoftwareInstruction>,
    pub git_status: String,
    pub git_diff_stat: String,
    pub git_changed_files: Vec<String>,
}

/// Shell check that a software-agent run should satisfy.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwareCheck {
    pub command: String,
}

#[cfg(feature = "native")]
impl SoftwareCheck {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }
}

/// Concrete plan for a software-agent run.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwarePlan {
    pub prompt: String,
    pub steps: Vec<String>,
    pub checks: Vec<SoftwareCheck>,
}

/// Result of applying a patch to a workspace.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwarePatch {
    pub path: PathBuf,
    pub success: bool,
    pub files: Vec<String>,
    pub stdout: String,
    pub stderr: String,
}

/// Result of running a software lifecycle check.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwareCheckResult {
    pub command: String,
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// Human and machine summary of a software-agent run.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwareSummary {
    pub prompt: String,
    pub changed_files: Vec<String>,
    pub checks: Vec<SoftwareCheckResult>,
    pub text: String,
}

/// Result of committing a verified software-agent change.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwareCommit {
    pub message: String,
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Result of opening a pull request for a verified software-agent change.
#[cfg(feature = "native")]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SoftwarePullRequest {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Local software lifecycle API for agents that build and maintain repositories.
#[cfg(feature = "native")]
#[derive(Debug, Clone)]
pub struct SoftwareWorkspace {
    root: PathBuf,
}

#[cfg(feature = "native")]
impl SoftwareWorkspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn inspect_repo(&self) -> Result<RepoInspection, AgenticHarnessError> {
        Ok(RepoInspection {
            root: self
                .root
                .canonicalize()
                .unwrap_or_else(|_| self.root.clone()),
            root_files: software_root_files(&self.root)?,
            project_files: software_project_files(&self.root),
            instructions: self.read_instructions()?,
            git_status: software_git_output(&self.root, &["status", "--short"], "git status"),
            git_diff_stat: software_git_output(&self.root, &["diff", "--stat"], "git diff --stat"),
            git_changed_files: software_git_changed_files(&self.root),
        })
    }

    pub fn read_instructions(&self) -> Result<Vec<SoftwareInstruction>, AgenticHarnessError> {
        let mut instructions = Vec::new();
        for relative in ["AGENTS.md", "CLAUDE.md"] {
            let path = self.root.join(relative);
            if path.exists() {
                instructions.push(SoftwareInstruction {
                    path: relative.to_string(),
                    content: fs::read_to_string(path)?,
                });
            }
        }
        Ok(instructions)
    }

    pub fn create_plan(
        &self,
        prompt: impl Into<String>,
        checks: Vec<SoftwareCheck>,
    ) -> Result<SoftwarePlan, AgenticHarnessError> {
        let prompt = prompt.into();
        let mut steps = vec![
            "Inspect repository state, project files, and instruction files.".to_string(),
            "Plan the smallest focused change that satisfies the prompt.".to_string(),
            "Apply edits as a patch or direct workspace change.".to_string(),
        ];
        if checks.is_empty() {
            steps.push("Record that no checks were configured.".to_string());
        } else {
            steps.push(format!(
                "Run checks: {}.",
                checks
                    .iter()
                    .map(|check| check.command.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ));
        }
        steps.push("Summarize changes, checks, and remaining risk.".to_string());
        Ok(SoftwarePlan {
            prompt,
            steps,
            checks,
        })
    }

    pub fn apply_patch(&self, patch: &Path) -> Result<SoftwarePatch, AgenticHarnessError> {
        let files = software_patch_files(patch);
        let output = Command::new("git")
            .arg("apply")
            .arg("--whitespace=nowarn")
            .arg(patch)
            .current_dir(&self.root)
            .output()?;
        Ok(SoftwarePatch {
            path: patch.to_path_buf(),
            success: output.status.success(),
            files,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    pub fn run_checks(
        &self,
        checks: &[SoftwareCheck],
    ) -> Result<Vec<SoftwareCheckResult>, AgenticHarnessError> {
        checks
            .iter()
            .map(|check| self.run_check(check))
            .collect::<Result<Vec<_>, _>>()
    }

    pub fn summarize_changes(
        &self,
        prompt: impl Into<String>,
        checks: &[SoftwareCheckResult],
    ) -> Result<SoftwareSummary, AgenticHarnessError> {
        let prompt = prompt.into();
        let changed_files = software_git_changed_files(&self.root);
        let mut text = String::new();
        text.push_str(&format!("Prompt: {prompt}\n"));
        text.push_str("Changed files:\n");
        if changed_files.is_empty() {
            text.push_str("- none\n");
        } else {
            for file in &changed_files {
                text.push_str(&format!("- {file}\n"));
            }
        }
        text.push_str("Checks:\n");
        if checks.is_empty() {
            text.push_str("- none\n");
        } else {
            for check in checks {
                text.push_str(&format!(
                    "- {}: {}\n",
                    check.command,
                    if check.success { "passed" } else { "failed" }
                ));
            }
        }
        Ok(SoftwareSummary {
            prompt,
            changed_files,
            checks: checks.to_vec(),
            text,
        })
    }

    pub fn repair_failure(
        &self,
        prompt: impl Into<String>,
        failed_checks: &[SoftwareCheckResult],
    ) -> Result<SoftwarePlan, AgenticHarnessError> {
        let prompt = prompt.into();
        let checks = failed_checks
            .iter()
            .map(|result| SoftwareCheck::new(result.command.clone()))
            .collect::<Vec<_>>();
        let mut steps = vec![
            "Inspect failed checks before editing.".to_string(),
            "Read the smallest source and test files related to the first failure.".to_string(),
        ];
        if failed_checks.is_empty() {
            steps.push("No failed checks were provided; inspect the repository and ask for the failing command.".to_string());
        } else {
            for result in failed_checks {
                steps.push(format!(
                    "Repair `{}` which exited with {:?}; stderr excerpt: {}",
                    result.command,
                    result.exit_code,
                    result.stderr.chars().take(240).collect::<String>()
                ));
            }
        }
        steps.push("Apply the smallest patch and rerun the failed checks.".to_string());
        steps.push("Summarize the fix and any remaining risk.".to_string());
        Ok(SoftwarePlan {
            prompt,
            steps,
            checks,
        })
    }

    pub fn commit(&self, message: &str) -> Result<SoftwareCommit, AgenticHarnessError> {
        let add = Command::new("git")
            .args(["add", "-A"])
            .current_dir(&self.root)
            .output()?;
        if !add.status.success() {
            return Ok(SoftwareCommit {
                message: message.to_string(),
                success: false,
                stdout: String::from_utf8_lossy(&add.stdout).to_string(),
                stderr: String::from_utf8_lossy(&add.stderr).to_string(),
            });
        }
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(&self.root)
            .output()?;
        Ok(SoftwareCommit {
            message: message.to_string(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    pub fn open_pull_request(&self) -> Result<SoftwarePullRequest, AgenticHarnessError> {
        let output = Command::new("gh")
            .args(["pr", "create", "--fill"])
            .current_dir(&self.root)
            .output()?;
        Ok(SoftwarePullRequest {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    fn run_check(&self, check: &SoftwareCheck) -> Result<SoftwareCheckResult, AgenticHarnessError> {
        let output = Command::new("sh")
            .arg("-c")
            .arg(&check.command)
            .current_dir(&self.root)
            .output()?;
        Ok(SoftwareCheckResult {
            command: check.command.clone(),
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }
}

#[cfg(feature = "native")]
fn software_root_files(root: &Path) -> Result<Vec<String>, AgenticHarnessError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target" | "node_modules") {
            continue;
        }
        let suffix = if entry.file_type()?.is_dir() { "/" } else { "" };
        files.push(format!("{name}{suffix}"));
    }
    files.sort();
    Ok(files)
}

#[cfg(feature = "native")]
fn software_project_files(root: &Path) -> Vec<String> {
    [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "package-lock.json",
        "pyproject.toml",
        "requirements.txt",
        "go.mod",
        "Makefile",
    ]
    .into_iter()
    .filter(|relative| root.join(relative).exists())
    .map(ToOwned::to_owned)
    .collect()
}

#[cfg(feature = "native")]
fn software_git_output(root: &Path, args: &[&str], label: &str) -> String {
    match Command::new("git").args(args).current_dir(root).output() {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.to_ascii_lowercase().contains("not a git repository") {
                format!("{label} failed: not a git repository")
            } else {
                format!("{label} failed: {}", stderr.trim())
            }
        }
        Err(err) => format!("{label} unavailable: {err}"),
    }
}

#[cfg(feature = "native")]
fn software_git_changed_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let diff = software_git_output(root, &["diff", "--name-only"], "git diff --name-only");
    if !diff.starts_with("git diff --name-only failed:")
        && !diff.starts_with("git diff --name-only unavailable:")
    {
        files.extend(
            diff.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    let status = software_git_output(root, &["status", "--short"], "git status");
    if !status.starts_with("git status failed:") && !status.starts_with("git status unavailable:") {
        for line in status.lines() {
            if line.len() < 4 {
                continue;
            }
            let mut file = line[3..].trim();
            if let Some((_, renamed_to)) = file.rsplit_once(" -> ") {
                file = renamed_to.trim();
            }
            let file = file.trim_matches('"');
            if !file.is_empty() && !files.iter().any(|existing| existing == file) {
                files.push(file.to_string());
            }
        }
    }
    files
}

#[cfg(feature = "native")]
fn software_patch_files(path: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for line in content.lines() {
        if let Some(file) = line.strip_prefix("+++ b/") {
            if !files.iter().any(|existing| existing == file) {
                files.push(file.to_string());
            }
        } else if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(file) = rest
                .split_whitespace()
                .nth(1)
                .and_then(|path| path.strip_prefix("b/"))
            {
                if !files.iter().any(|existing| existing == file) {
                    files.push(file.to_string());
                }
            }
        }
    }
    files
}

/// Explicit host command exposed to native agents.
#[derive(Clone)]
pub struct CommandDef {
    name: String,
    handler: Arc<CommandHandler>,
}

impl CommandDef {
    pub fn new<F>(name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(&[String]) -> Result<ShellOutput, AgenticHarnessError> + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            handler: Arc::new(handler),
        }
    }

    #[cfg(feature = "native")]
    pub fn passthrough(name: impl Into<String>) -> Self {
        Self::passthrough_with_env(name, BTreeMap::new())
    }

    #[cfg(feature = "native")]
    pub fn passthrough_with_env(name: impl Into<String>, env: BTreeMap<String, String>) -> Self {
        let name = name.into();
        let program = name.clone();
        Self::new(name, move |args| {
            let output = Command::new(&program)
                .args(args)
                .env_clear()
                .envs(default_shell_env())
                .envs(env.clone())
                .output()?;
            Ok(shell_output(output))
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    fn execute(&self, args: &[String]) -> Result<ShellOutput, AgenticHarnessError> {
        (self.handler)(args)
    }
}

/// JSON-schema-compatible tool metadata passed to model clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// Custom Rust tool exposed to model calls or invoked directly by handlers.
#[derive(Clone)]
pub struct ToolDef {
    spec: ToolSpec,
    handler: Arc<ToolHandler>,
}

impl ToolDef {
    pub fn new<F>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        handler: F,
    ) -> Self
    where
        F: Fn(Value) -> Result<String, AgenticHarnessError> + Send + Sync + 'static,
    {
        Self {
            spec: ToolSpec {
                name: name.into(),
                description: description.into(),
                parameters,
            },
            handler: Arc::new(handler),
        }
    }

    pub fn spec(&self) -> &ToolSpec {
        &self.spec
    }

    pub fn name(&self) -> &str {
        &self.spec.name
    }

    fn execute(&self, args: Value) -> Result<String, AgenticHarnessError> {
        (self.handler)(args)
    }
}

/// Minimal MCP client contract used to adapt MCP tools into Agentic Harness tools.
pub trait McpClient: Send + Sync {
    fn list_tools(&self) -> Result<Vec<McpTool>, AgenticHarnessError>;
    fn call_tool(&self, name: &str, args: Value) -> Result<Value, AgenticHarnessError>;
}

/// MCP tool metadata returned by a server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub input_schema: Value,
}

/// MCP HTTP transport mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    StreamableHttp,
    Sse,
}

/// Options for connecting an MCP HTTP server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerOptions {
    pub url: String,
    pub transport: McpTransport,
    pub headers: BTreeMap<String, String>,
    pub client_name: String,
    pub client_version: String,
}

impl McpServerOptions {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            transport: McpTransport::StreamableHttp,
            headers: BTreeMap::new(),
            client_name: "agentic-harness".to_string(),
            client_version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn transport(mut self, transport: McpTransport) -> Self {
        self.transport = transport;
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn client_name(mut self, value: impl Into<String>) -> Self {
        self.client_name = value.into();
        self
    }

    pub fn client_version(mut self, value: impl Into<String>) -> Self {
        self.client_version = value.into();
        self
    }
}

/// Connected MCP server with tools converted into Agentic Harness custom tools.
pub struct McpServerConnection {
    pub name: String,
    pub tools: Vec<ToolDef>,
}

impl McpServerConnection {
    pub fn close(&self) -> Result<(), AgenticHarnessError> {
        Ok(())
    }
}

/// Connect to an MCP streamable-HTTP endpoint and expose its tools as [`ToolDef`]s.
#[cfg(feature = "native")]
pub fn connect_mcp_server(
    name: impl Into<String>,
    options: McpServerOptions,
) -> Result<McpServerConnection, AgenticHarnessError> {
    let name = name.into();
    let client = Arc::new(McpHttpClient::new(options));
    client.initialize()?;
    let tools = create_mcp_tools(&name, client.clone(), client.list_tools()?)?;
    Ok(McpServerConnection { tools, name })
}

/// Convert any MCP client implementation into Agentic Harness custom tools.
pub fn mcp_tools_from_client<C>(
    server_name: impl AsRef<str>,
    client: C,
) -> Result<Vec<ToolDef>, AgenticHarnessError>
where
    C: McpClient + 'static,
{
    let client = Arc::new(client);
    let tools = client.list_tools()?;
    create_mcp_tools(server_name.as_ref(), client, tools)
}

#[cfg(feature = "native")]
struct McpHttpClient {
    endpoint: String,
    transport: McpTransport,
    headers: BTreeMap<String, String>,
    client_name: String,
    client_version: String,
    http: reqwest::blocking::Client,
    next_id: AtomicU64,
    session_id: Mutex<Option<String>>,
    legacy_sse_endpoint: Mutex<Option<String>>,
}

#[cfg(feature = "native")]
impl McpHttpClient {
    fn new(options: McpServerOptions) -> Self {
        Self {
            endpoint: options.url,
            transport: options.transport,
            headers: options.headers,
            client_name: options.client_name,
            client_version: options.client_version,
            http: reqwest::blocking::Client::new(),
            next_id: AtomicU64::new(1),
            session_id: Mutex::new(None),
            legacy_sse_endpoint: Mutex::new(None),
        }
    }

    fn initialize(&self) -> Result<(), AgenticHarnessError> {
        self.rpc(
            "initialize",
            json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": self.client_name,
                    "version": self.client_version
                }
            }),
        )?;
        Ok(())
    }

    fn rpc(&self, method: &str, params: Value) -> Result<Value, AgenticHarnessError> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let body = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let endpoint = self.rpc_endpoint()?;

        let mut request = self
            .http
            .post(endpoint)
            .header("accept", "application/json, text/event-stream")
            .header("content-type", "application/json")
            .json(&body);
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }
        if let Some(session_id) = self
            .session_id
            .lock()
            .map_err(|_| AgenticHarnessError::Mcp {
                message: "MCP session lock poisoned".to_string(),
            })?
            .clone()
        {
            request = request.header("mcp-session-id", session_id);
        }

        let response = request.send()?;
        if let Some(session_id) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string)
        {
            *self
                .session_id
                .lock()
                .map_err(|_| AgenticHarnessError::Mcp {
                    message: "MCP session lock poisoned".to_string(),
                })? = Some(session_id);
        }

        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            return Err(AgenticHarnessError::Mcp {
                message: format!("MCP HTTP request failed with status {status}: {text}"),
            });
        }

        let envelope = parse_mcp_response_body(&text)?;
        if let Some(error) = envelope.get("error") {
            return Err(AgenticHarnessError::Mcp {
                message: serde_json::to_string(error).unwrap_or_else(|_| error.to_string()),
            });
        }
        envelope
            .get("result")
            .cloned()
            .ok_or_else(|| AgenticHarnessError::Mcp {
                message: "MCP response did not include result".to_string(),
            })
    }

    fn rpc_endpoint(&self) -> Result<String, AgenticHarnessError> {
        match self.transport {
            McpTransport::StreamableHttp => Ok(self.endpoint.clone()),
            McpTransport::Sse => self.legacy_sse_rpc_endpoint(),
        }
    }

    fn legacy_sse_rpc_endpoint(&self) -> Result<String, AgenticHarnessError> {
        if let Some(endpoint) = self
            .legacy_sse_endpoint
            .lock()
            .map_err(|_| AgenticHarnessError::Mcp {
                message: "MCP legacy SSE endpoint lock poisoned".to_string(),
            })?
            .clone()
        {
            return Ok(endpoint);
        }

        let mut request = self
            .http
            .get(&self.endpoint)
            .header("accept", "text/event-stream");
        for (name, value) in &self.headers {
            request = request.header(name, value);
        }

        let response = request.send()?;
        let status = response.status();
        let text = response.text()?;
        if !status.is_success() {
            return Err(AgenticHarnessError::Mcp {
                message: format!("MCP legacy SSE handshake failed with status {status}: {text}"),
            });
        }

        let endpoint = resolve_legacy_sse_endpoint(&self.endpoint, parse_sse_endpoint(&text)?)?;
        *self
            .legacy_sse_endpoint
            .lock()
            .map_err(|_| AgenticHarnessError::Mcp {
                message: "MCP legacy SSE endpoint lock poisoned".to_string(),
            })? = Some(endpoint.clone());
        Ok(endpoint)
    }
}

#[cfg(feature = "native")]
impl McpClient for McpHttpClient {
    fn list_tools(&self) -> Result<Vec<McpTool>, AgenticHarnessError> {
        let result = self.rpc("tools/list", json!({}))?;
        let tools = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| AgenticHarnessError::Mcp {
                message: "MCP tools/list response did not include result.tools".to_string(),
            })?;

        tools
            .iter()
            .map(|tool| {
                let name = tool
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| AgenticHarnessError::Mcp {
                        message: "MCP tool did not include a string name".to_string(),
                    })?
                    .to_string();
                let title = tool
                    .get("title")
                    .or_else(|| tool.pointer("/annotations/title"))
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let description = tool
                    .get("description")
                    .and_then(Value::as_str)
                    .map(ToString::to_string);
                let input_schema = tool
                    .get("inputSchema")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Ok(McpTool {
                    name,
                    title,
                    description,
                    input_schema,
                })
            })
            .collect()
    }

    fn call_tool(&self, name: &str, args: Value) -> Result<Value, AgenticHarnessError> {
        self.rpc(
            "tools/call",
            json!({
                "name": name,
                "arguments": args
            }),
        )
    }
}

fn create_mcp_tools(
    server_name: &str,
    client: Arc<dyn McpClient>,
    tools: Vec<McpTool>,
) -> Result<Vec<ToolDef>, AgenticHarnessError> {
    let mut seen = BTreeMap::new();
    let mut defs = Vec::new();
    for tool in tools {
        let tool_name = mcp_tool_name(server_name, &tool.name);
        if seen.insert(tool_name.clone(), ()).is_some() {
            return Err(AgenticHarnessError::Mcp {
                message: format!(
                    "MCP tools from server \"{server_name}\" produced duplicate tool name \"{tool_name}\"."
                ),
            });
        }

        let original_name = tool.name.clone();
        let tool_client = client.clone();
        defs.push(ToolDef::new(
            tool_name,
            mcp_tool_description(server_name, &tool),
            normalize_mcp_input_schema(tool.input_schema),
            move |args| {
                let result = tool_client.call_tool(&original_name, args)?;
                if result
                    .get("isError")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    return Err(AgenticHarnessError::Mcp {
                        message: format_mcp_result(&result),
                    });
                }
                Ok(format_mcp_result(&result))
            },
        ));
    }
    Ok(defs)
}

fn mcp_tool_name(server_name: &str, tool_name: &str) -> String {
    format!(
        "mcp__{}__{}",
        sanitize_mcp_name_part(server_name),
        sanitize_mcp_name_part(tool_name)
    )
}

fn sanitize_mcp_name_part(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed.to_string()
    }
}

fn mcp_tool_description(server_name: &str, tool: &McpTool) -> String {
    let mut parts = vec![format!(
        "MCP tool \"{}\" from server \"{server_name}\".",
        tool.name
    )];
    if let Some(title) = &tool.title {
        if title != &tool.name {
            parts.push(format!("Title: {title}."));
        }
    }
    if let Some(description) = &tool.description {
        if !description.is_empty() {
            parts.push(description.clone());
        }
    }
    parts.join(" ")
}

fn normalize_mcp_input_schema(schema: Value) -> Value {
    let mut object = match schema {
        Value::Object(object) => object,
        _ => serde_json::Map::new(),
    };
    object
        .entry("type".to_string())
        .or_insert_with(|| json!("object"));
    object
        .entry("properties".to_string())
        .or_insert_with(|| json!({}));
    Value::Object(object)
}

#[cfg(feature = "native")]
fn parse_mcp_response_body(body: &str) -> Result<Value, AgenticHarnessError> {
    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return Ok(value);
    }

    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        return serde_json::from_str(data).map_err(|err| AgenticHarnessError::Mcp {
            message: format!("MCP SSE data contained invalid JSON: {err}"),
        });
    }

    Err(AgenticHarnessError::Mcp {
        message: "MCP response was neither JSON nor SSE data JSON".to_string(),
    })
}

#[cfg(feature = "native")]
fn parse_sse_endpoint(body: &str) -> Result<String, AgenticHarnessError> {
    let mut event_name: Option<String> = None;
    let mut data_lines = Vec::new();

    for line in body.lines().chain(std::iter::once("")) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            if event_name.as_deref() == Some("endpoint") && !data_lines.is_empty() {
                return Ok(data_lines.join("\n").trim().to_string());
            }
            event_name = None;
            data_lines.clear();
            continue;
        }
        if line.starts_with(':') {
            continue;
        }
        if let Some(event) = line.strip_prefix("event:") {
            event_name = Some(event.trim().to_string());
            continue;
        }
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim().to_string());
        }
    }

    Err(AgenticHarnessError::Mcp {
        message: "MCP legacy SSE handshake did not include an endpoint event".to_string(),
    })
}

#[cfg(feature = "native")]
fn resolve_legacy_sse_endpoint(
    base: &str,
    endpoint: String,
) -> Result<String, AgenticHarnessError> {
    if endpoint.trim().is_empty() {
        return Err(AgenticHarnessError::Mcp {
            message: "MCP legacy SSE endpoint event was empty".to_string(),
        });
    }
    if reqwest::Url::parse(&endpoint).is_ok() {
        return Ok(endpoint);
    }
    let base = reqwest::Url::parse(base).map_err(|err| AgenticHarnessError::Mcp {
        message: format!("MCP legacy SSE base URL was invalid: {err}"),
    })?;
    base.join(&endpoint)
        .map(|url| url.to_string())
        .map_err(|err| AgenticHarnessError::Mcp {
            message: format!("MCP legacy SSE endpoint URL was invalid: {err}"),
        })
}

fn format_mcp_result(result: &Value) -> String {
    let mut parts = Vec::new();

    if let Some(structured) = result.get("structuredContent") {
        parts.push(format!(
            "Structured content:\n{}",
            serde_json::to_string_pretty(structured).unwrap_or_else(|_| structured.to_string())
        ));
    }

    if let Some(content) = result.get("content").and_then(Value::as_array) {
        for item in content {
            match item.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        parts.push(text.to_string());
                    }
                }
                Some("image") => parts.push(format!(
                    "[Image: {}, {} base64 chars]",
                    item.get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream"),
                    item.get("data").and_then(Value::as_str).unwrap_or("").len()
                )),
                Some("audio") => parts.push(format!(
                    "[Audio: {}, {} base64 chars]",
                    item.get("mimeType")
                        .and_then(Value::as_str)
                        .unwrap_or("application/octet-stream"),
                    item.get("data").and_then(Value::as_str).unwrap_or("").len()
                )),
                Some("resource") => {
                    if let Some(resource) = item.get("resource") {
                        let uri = resource
                            .get("uri")
                            .and_then(Value::as_str)
                            .unwrap_or("(unknown)");
                        if let Some(text) = resource.get("text").and_then(Value::as_str) {
                            parts.push(format!("[Resource: {uri}]\n{text}"));
                        } else {
                            parts.push(format!(
                                "[Resource: {uri}, {} base64 chars]",
                                resource
                                    .get("blob")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .len()
                            ));
                        }
                    }
                }
                Some("resource_link") => {
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("resource");
                    let uri = item.get("uri").and_then(Value::as_str).unwrap_or("");
                    let description = item
                        .get("description")
                        .and_then(Value::as_str)
                        .map(|value| format!(" - {value}"))
                        .unwrap_or_default();
                    parts.push(format!("[Resource link: {name} ({uri}){description}]"));
                }
                _ => parts.push(serde_json::to_string(item).unwrap_or_else(|_| item.to_string())),
            }
        }
    }

    if parts.is_empty() {
        if let Some(tool_result) = result.get("toolResult") {
            parts.push(
                serde_json::to_string_pretty(tool_result)
                    .unwrap_or_else(|_| tool_result.to_string()),
            );
        }
    }

    let text = parts
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if text.is_empty() {
        "(MCP tool returned no content)".to_string()
    } else {
        text
    }
}

/// One grep match returned by [`AgentContext::grep`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

/// Runtime provider settings for a family of model IDs.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSettings {
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub api_key_env: Option<String>,
}

impl ProviderSettings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = Some(base_url.into());
        self
    }

    pub fn header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(key.into(), value.into());
        self
    }

    pub fn api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    pub fn api_key_env(mut self, env_var: impl Into<String>) -> Self {
        self.api_key_env = Some(env_var.into());
        self
    }

    pub fn resolved_api_key(&self) -> Option<String> {
        self.api_key.clone().or_else(|| {
            self.api_key_env
                .as_deref()
                .and_then(|name| std::env::var(name).ok())
        })
    }

    pub fn merged(&self, overrides: &Self) -> Self {
        let mut headers = self.headers.clone();
        headers.extend(overrides.headers.clone());
        Self {
            base_url: overrides.base_url.clone().or_else(|| self.base_url.clone()),
            headers,
            api_key: overrides.api_key.clone().or_else(|| self.api_key.clone()),
            api_key_env: overrides
                .api_key_env
                .clone()
                .or_else(|| self.api_key_env.clone()),
        }
    }
}

/// Runtime provider settings keyed by provider name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProvidersConfig {
    providers: BTreeMap<String, ProviderSettings>,
}

impl ProvidersConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(
        &mut self,
        provider: impl Into<String>,
        settings: ProviderSettings,
    ) -> Option<ProviderSettings> {
        self.providers.insert(provider.into(), settings)
    }

    pub fn get(&self, provider: &str) -> Option<&ProviderSettings> {
        self.providers.get(provider)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &ProviderSettings)> {
        self.providers.iter()
    }

    pub fn merged(&self, overrides: &Self) -> Self {
        let mut merged = self.clone();
        for (provider, settings) in &overrides.providers {
            let settings = merged
                .providers
                .get(provider)
                .map(|base| base.merged(settings))
                .unwrap_or_else(|| settings.clone());
            merged.providers.insert(provider.clone(), settings);
        }
        merged
    }
}

/// File-loadable runtime model/provider configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRuntimeConfig {
    pub default_model: Option<String>,
    #[serde(default)]
    pub providers: ProvidersConfig,
    #[serde(default)]
    pub openai_compatible_models: Vec<String>,
}

impl AgentRuntimeConfig {
    pub fn from_json_str(input: &str) -> Result<Self, AgenticHarnessError> {
        serde_json::from_str(input).map_err(AgenticHarnessError::from)
    }

    #[cfg(feature = "native")]
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, AgenticHarnessError> {
        Self::from_json_str(&fs::read_to_string(path)?)
    }

    #[cfg(feature = "native")]
    pub fn discover_path(workspace: impl AsRef<Path>) -> Option<PathBuf> {
        let workspace = workspace.as_ref();
        [
            workspace.join("agentic-harness.json"),
            workspace.join(".agentic-harness/config.json"),
        ]
        .into_iter()
        .find(|path| path.exists())
    }

    #[cfg(feature = "native")]
    pub fn from_workspace(
        workspace: impl AsRef<Path>,
    ) -> Result<Option<Self>, AgenticHarnessError> {
        Self::discover_path(workspace)
            .map(Self::from_path)
            .transpose()
    }
}

/// Parsed `provider/model-id` model config.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConfig {
    pub provider: String,
    pub model: String,
}

impl ModelConfig {
    pub fn parse(value: impl AsRef<str>) -> Result<Self, AgenticHarnessError> {
        let value = value.as_ref();
        let Some((provider, model)) = value.split_once('/') else {
            return Err(AgenticHarnessError::InvalidModel {
                model: value.to_string(),
                message: "expected provider/model-id".to_string(),
            });
        };
        if provider.trim().is_empty() || model.trim().is_empty() {
            return Err(AgenticHarnessError::InvalidModel {
                model: value.to_string(),
                message: "provider and model id must be non-empty".to_string(),
            });
        }
        Ok(Self {
            provider: provider.to_string(),
            model: model.to_string(),
        })
    }

    pub fn as_key(&self) -> String {
        format!("{}/{}", self.provider, self.model)
    }
}

/// Options for configured model calls.
#[derive(Clone, Default)]
pub struct PromptOptions {
    pub role: Option<String>,
    pub model: Option<String>,
    pub tools: Vec<ToolDef>,
    pub compaction: Option<CompactionSettings>,
    pub result_schema: Option<Value>,
}

impl PromptOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    pub fn tool(mut self, tool: ToolDef) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn compaction(mut self, settings: CompactionSettings) -> Self {
        self.compaction = Some(settings);
        self
    }

    pub fn result_schema(mut self, schema: Value) -> Self {
        self.result_schema = Some(schema);
        self
    }
}

/// Token-budget settings for automatic prompt-time session compaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSettings {
    pub enabled: bool,
    pub context_window_tokens: usize,
    pub reserve_tokens: usize,
    pub keep_recent_messages: usize,
}

impl Default for CompactionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            context_window_tokens: 128_000,
            reserve_tokens: 16_384,
            keep_recent_messages: 12,
        }
    }
}

impl CompactionSettings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn context_window_tokens(mut self, value: usize) -> Self {
        self.context_window_tokens = value;
        self
    }

    pub fn reserve_tokens(mut self, value: usize) -> Self {
        self.reserve_tokens = value;
        self
    }

    pub fn keep_recent_messages(mut self, value: usize) -> Self {
        self.keep_recent_messages = value;
        self
    }
}

/// File-backed session history store for native agents.
#[cfg(feature = "native")]
#[derive(Debug, Clone)]
pub struct FileSessionStore {
    root: PathBuf,
}

#[cfg(feature = "native")]
impl FileSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        self.root.join(format!("{:016x}.json", hasher.finish()))
    }
}

#[cfg(feature = "native")]
impl SessionStore for FileSessionStore {
    fn load(&self, key: &str) -> Result<Option<SessionData>, AgenticHarnessError> {
        let path = self.path_for(key);
        if !path.exists() {
            return Ok(None);
        }
        let content = fs::read_to_string(path)?;
        if let Ok(data) = serde_json::from_str::<SessionData>(&content) {
            return Ok(Some(data));
        }
        let legacy_history: Vec<ModelMessage> = serde_json::from_str(&content)?;
        Ok(Some(SessionData::from_messages(legacy_history)))
    }

    fn save(&self, key: &str, data: &SessionData) -> Result<(), AgenticHarnessError> {
        fs::create_dir_all(&self.root)?;
        fs::write(self.path_for(key), serde_json::to_string_pretty(data)?)?;
        Ok(())
    }

    fn delete(&self, key: &str) -> Result<(), AgenticHarnessError> {
        let path = self.path_for(key);
        if path.exists() {
            fs::remove_file(path)?;
        }
        Ok(())
    }
}

/// Details attached to a compaction record.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionDetails {
    pub read_files: Vec<String>,
    pub modified_files: Vec<String>,
}

/// Options for [`Session::compact_with_summary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionOptions {
    pub keep_recent_messages: usize,
    pub tokens_before: usize,
    pub details: Option<CompactionDetails>,
}

impl Default for CompactionOptions {
    fn default() -> Self {
        Self {
            keep_recent_messages: 12,
            tokens_before: 0,
            details: None,
        }
    }
}

impl CompactionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn keep_recent_messages(mut self, value: usize) -> Self {
        self.keep_recent_messages = value;
        self
    }

    pub fn tokens_before(mut self, value: usize) -> Self {
        self.tokens_before = value;
        self
    }

    pub fn details(mut self, details: CompactionDetails) -> Self {
        self.details = Some(details);
        self
    }
}

/// Persisted session data with message, compaction, and branch-summary entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionData {
    pub version: u8,
    pub entries: Vec<SessionEntry>,
    pub leaf_id: Option<String>,
    pub metadata: BTreeMap<String, Value>,
    pub created_at_unix_ms: u64,
    pub updated_at_unix_ms: u64,
}

impl Default for SessionData {
    fn default() -> Self {
        Self::empty()
    }
}

impl SessionData {
    pub fn empty() -> Self {
        let now = now_unix_ms();
        Self {
            version: 2,
            entries: Vec::new(),
            leaf_id: None,
            metadata: BTreeMap::new(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
        }
    }

    pub fn from_messages(messages: Vec<ModelMessage>) -> Self {
        let mut data = Self::empty();
        for message in messages {
            data.append_message(message, None);
        }
        data
    }

    pub fn context_messages(&self) -> Vec<ModelMessage> {
        let path = self.active_entries();
        let Some(compaction_index) = path.iter().rposition(|entry| entry.is_compaction()) else {
            return path_entries_to_context(&path);
        };
        let compaction = match path[compaction_index] {
            SessionEntry::Compaction {
                summary,
                first_kept_entry_id,
                ..
            } => (summary, first_kept_entry_id),
            _ => unreachable!(),
        };
        let kept_start = path
            .iter()
            .position(|entry| entry.id() == compaction.1)
            .unwrap_or(compaction_index + 1);
        let mut context = vec![ModelMessage {
            role: "user".to_string(),
            content: context_summary_text(compaction.0),
        }];
        context.extend(path_entries_to_context(&path[kept_start..compaction_index]));
        context.extend(path_entries_to_context(&path[(compaction_index + 1)..]));
        context
    }

    fn active_entries(&self) -> Vec<&SessionEntry> {
        let by_id = self
            .entries
            .iter()
            .map(|entry| (entry.id(), entry))
            .collect::<BTreeMap<_, _>>();
        let mut path = Vec::new();
        let mut current_id = self.leaf_id.as_deref();
        while let Some(id) = current_id {
            let Some(entry) = by_id.get(id).copied() else {
                break;
            };
            path.push(entry);
            current_id = entry.parent_id();
        }
        path.reverse();
        path
    }

    fn append_message(&mut self, message: ModelMessage, source: Option<String>) -> String {
        let id = self.next_entry_id();
        let entry = SessionEntry::Message {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp_unix_ms: now_unix_ms(),
            message,
            source,
        };
        self.append_entry(entry);
        id
    }

    fn append_compaction(
        &mut self,
        summary: String,
        options: CompactionOptions,
    ) -> Result<String, AgenticHarnessError> {
        let path = self.active_entries();
        if path.is_empty() {
            return Err(AgenticHarnessError::Handler {
                message: "cannot compact an empty session".to_string(),
            });
        }
        let message_entries = path
            .iter()
            .filter(|entry| entry.is_message() || entry.is_branch_summary())
            .copied()
            .collect::<Vec<_>>();
        let keep = options.keep_recent_messages.min(message_entries.len());
        let first_kept_entry_id = message_entries
            .get(message_entries.len().saturating_sub(keep))
            .or_else(|| message_entries.last())
            .map(|entry| entry.id().to_string())
            .ok_or_else(|| AgenticHarnessError::Handler {
                message: "cannot compact a session with no message entries".to_string(),
            })?;
        let id = self.next_entry_id();
        let entry = SessionEntry::Compaction {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp_unix_ms: now_unix_ms(),
            summary,
            first_kept_entry_id,
            tokens_before: options.tokens_before,
            details: options.details,
        };
        self.append_entry(entry);
        Ok(id)
    }

    fn append_branch_summary(
        &mut self,
        summary: String,
        from_id: String,
        details: Option<Value>,
    ) -> String {
        let id = self.next_entry_id();
        let entry = SessionEntry::BranchSummary {
            id: id.clone(),
            parent_id: self.leaf_id.clone(),
            timestamp_unix_ms: now_unix_ms(),
            from_id,
            summary,
            details,
        };
        self.append_entry(entry);
        id
    }

    fn append_entry(&mut self, entry: SessionEntry) {
        self.leaf_id = Some(entry.id().to_string());
        self.updated_at_unix_ms = now_unix_ms();
        self.entries.push(entry);
    }

    fn next_entry_id(&self) -> String {
        let mut index = self.entries.len() + 1;
        loop {
            let id = format!("entry-{index}");
            if self.entries.iter().all(|entry| entry.id() != id) {
                return id;
            }
            index += 1;
        }
    }
}

/// One persisted session entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum SessionEntry {
    Message {
        id: String,
        parent_id: Option<String>,
        timestamp_unix_ms: u64,
        message: ModelMessage,
        source: Option<String>,
    },
    Compaction {
        id: String,
        parent_id: Option<String>,
        timestamp_unix_ms: u64,
        summary: String,
        first_kept_entry_id: String,
        tokens_before: usize,
        details: Option<CompactionDetails>,
    },
    BranchSummary {
        id: String,
        parent_id: Option<String>,
        timestamp_unix_ms: u64,
        from_id: String,
        summary: String,
        details: Option<Value>,
    },
}

impl SessionEntry {
    fn id(&self) -> &str {
        match self {
            Self::Message { id, .. }
            | Self::Compaction { id, .. }
            | Self::BranchSummary { id, .. } => id,
        }
    }

    fn parent_id(&self) -> Option<&str> {
        match self {
            Self::Message { parent_id, .. }
            | Self::Compaction { parent_id, .. }
            | Self::BranchSummary { parent_id, .. } => parent_id.as_deref(),
        }
    }

    fn is_message(&self) -> bool {
        matches!(self, Self::Message { .. })
    }

    fn is_branch_summary(&self) -> bool {
        matches!(self, Self::BranchSummary { .. })
    }

    fn is_compaction(&self) -> bool {
        matches!(self, Self::Compaction { .. })
    }
}

fn path_entries_to_context(entries: &[&SessionEntry]) -> Vec<ModelMessage> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            SessionEntry::Message { message, .. } => Some(message.clone()),
            SessionEntry::BranchSummary { summary, .. } => Some(ModelMessage {
                role: "user".to_string(),
                content: branch_summary_text(summary),
            }),
            SessionEntry::Compaction { .. } => None,
        })
        .collect()
}

fn entries_to_compaction_context(entries: &[&SessionEntry]) -> Vec<ModelMessage> {
    entries
        .iter()
        .map(|entry| match entry {
            SessionEntry::Message { message, .. } => message.clone(),
            SessionEntry::BranchSummary { summary, .. } => ModelMessage {
                role: "user".to_string(),
                content: branch_summary_text(summary),
            },
            SessionEntry::Compaction { summary, .. } => ModelMessage {
                role: "user".to_string(),
                content: context_summary_text(summary),
            },
        })
        .collect()
}

fn context_summary_text(summary: &str) -> String {
    if summary.starts_with("[Context Summary]") {
        summary.to_string()
    } else {
        format!("[Context Summary]\n\n{summary}")
    }
}

fn branch_summary_text(summary: &str) -> String {
    if summary.starts_with("[Branch Summary]") {
        summary.to_string()
    } else {
        format!("[Branch Summary]\n\n{summary}")
    }
}

fn automatic_compaction_system_prompt() -> &'static str {
    "Summarize this Agentic Harness session for context compaction. Preserve user requirements, decisions, files, commands, errors, tool results, and next steps. Write concise text only and do not call tools."
}

fn serialize_messages_for_compaction(messages: &[ModelMessage]) -> String {
    messages
        .iter()
        .map(|message| format!("{}:\n{}", message.role, message.content))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

fn estimate_context_tokens(messages: &[ModelMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

fn estimate_message_tokens(message: &ModelMessage) -> usize {
    estimate_text_tokens(&message.role) + estimate_text_tokens(&message.content) + 4
}

fn estimate_text_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// One message in a native Agentic Harness session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: String,
    pub content: String,
}

/// Provider-agnostic model request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub system: String,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolSpec>,
}

/// Tool call requested by a model response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}

/// Text response returned by a model client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptResponse {
    pub text: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

impl PromptResponse {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tool_calls: Vec::new(),
        }
    }

    pub fn with_tool_calls(text: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            text: text.into(),
            tool_calls,
        }
    }

    /// Extract the last `---RESULT_START---` / `---RESULT_END---` JSON block.
    pub fn result_json<T: DeserializeOwned>(&self) -> Result<T, AgenticHarnessError> {
        let block = extract_last_result_block(&self.text)?;
        serde_json::from_str(block).map_err(|err| AgenticHarnessError::ResultExtraction {
            message: format!("Result block contains invalid JSON: {err}"),
        })
    }

    /// Extract the last result block as raw text.
    pub fn result_text(&self) -> Result<String, AgenticHarnessError> {
        Ok(extract_last_result_block(&self.text)?.to_string())
    }
}

/// Minimal synchronous model integration point for native Rust agents.
///
/// Agentic Harness does not force a provider here. Users can implement this trait for an
/// OpenAI, Anthropic, local, test, or gateway-backed client and keep secrets in
/// Rust code instead of prompt context.
pub trait ModelClient: Send + Sync {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError>;
}

/// Raw response returned by an [`HttpModelTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpModelTransportResponse {
    pub status: u16,
    pub body: String,
}

/// Transport used by [`OpenAiCompatibleModel`] to call a chat-completions API.
///
/// Native agents use the built-in reqwest transport. Worker/WASM adapters can
/// provide a platform transport that forwards through the host runtime's fetch
/// API without enabling native networking.
pub trait HttpModelTransport: Send + Sync {
    fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
    ) -> Result<HttpModelTransportResponse, AgenticHarnessError>;
}

#[cfg(feature = "native")]
#[derive(Debug, Clone)]
struct ReqwestModelTransport {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "native")]
impl Default for ReqwestModelTransport {
    fn default() -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
        }
    }
}

#[cfg(feature = "native")]
impl HttpModelTransport for ReqwestModelTransport {
    fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
    ) -> Result<HttpModelTransportResponse, AgenticHarnessError> {
        let mut request = self.client.post(url).json(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send()?;
        let status = response.status().as_u16();
        let body = response.text()?;
        Ok(HttpModelTransportResponse { status, body })
    }
}

/// OpenAI-compatible chat/completions provider client.
pub struct OpenAiCompatibleModel {
    model: String,
    settings: ProviderSettings,
    transport: Arc<dyn HttpModelTransport>,
}

impl OpenAiCompatibleModel {
    #[cfg(feature = "native")]
    pub fn new(model: impl Into<String>, settings: ProviderSettings) -> Self {
        Self {
            model: model.into(),
            settings,
            transport: Arc::new(ReqwestModelTransport::default()),
        }
    }

    pub fn with_transport<T: HttpModelTransport + 'static>(
        model: impl Into<String>,
        settings: ProviderSettings,
        transport: T,
    ) -> Self {
        Self {
            model: model.into(),
            settings,
            transport: Arc::new(transport),
        }
    }

    #[cfg(feature = "native")]
    pub fn from_model_config(config: ModelConfig, settings: ProviderSettings) -> Self {
        Self::new(config.model, settings)
    }

    pub fn endpoint(&self) -> String {
        let base = self
            .settings
            .base_url
            .as_deref()
            .unwrap_or("https://api.openai.com/v1")
            .trim_end_matches('/');
        format!("{base}/chat/completions")
    }

    pub fn request_json(&self, request: &ModelRequest) -> Value {
        let mut messages = Vec::new();
        if !request.system.trim().is_empty() {
            messages.push(json!({
                "role": "system",
                "content": request.system
            }));
        }
        for message in &request.messages {
            messages.push(json!({
                "role": message.role,
                "content": message.content
            }));
        }

        json!({
            "model": self.model,
            "messages": messages,
            "stream": false,
            "tools": request.tools.iter().map(openai_tool_spec).collect::<Vec<_>>()
        })
    }

    pub fn parse_response(&self, body: &str) -> Result<PromptResponse, AgenticHarnessError> {
        let value: Value = serde_json::from_str(body)?;
        let message = value
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .and_then(|choice| choice.get("message"))
            .ok_or_else(|| AgenticHarnessError::Provider {
                message: "provider response did not include choices[0].message".to_string(),
            })?;
        let text = message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let tool_calls = parse_openai_tool_calls(message)?;
        if text.is_empty() && tool_calls.is_empty() {
            return Err(AgenticHarnessError::Provider {
                message: "provider response did not include message content or tool calls"
                    .to_string(),
            });
        }
        Ok(PromptResponse::with_tool_calls(text, tool_calls))
    }
}

impl ModelClient for OpenAiCompatibleModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let body = self.request_json(&request);
        let mut headers = self.settings.headers.clone();
        if let Some(api_key) = self.settings.resolved_api_key() {
            headers.insert("Authorization".to_string(), format!("Bearer {api_key}"));
        }

        let response = self
            .transport
            .post_json(&self.endpoint(), &headers, &body)?;
        if !(200..300).contains(&response.status) {
            return Err(AgenticHarnessError::Provider {
                message: format!("provider returned {}: {}", response.status, response.body),
            });
        }

        self.parse_response(&response.body)
    }
}

/// Runtime environment contract for native sandbox connectors.
pub trait SessionEnv: Send + Sync {
    fn exec(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError>;
    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError>;
    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError>;
    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError>;
    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError>;
    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError>;
    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError>;
    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError>;
    fn cwd(&self) -> &Path;
    fn resolve_path(&self, path: &str) -> PathBuf;
}

/// Empty in-memory session environment for isolated file/search operations.
#[derive(Debug, Clone)]
pub struct MemorySessionEnv {
    cwd: PathBuf,
    fs: Arc<Mutex<MemoryFs>>,
}

#[derive(Debug, Default)]
struct MemoryFs {
    files: BTreeMap<PathBuf, Vec<u8>>,
    dirs: BTreeSet<PathBuf>,
}

impl MemorySessionEnv {
    pub fn new(cwd: impl AsRef<Path>) -> Self {
        let cwd = normalize_memory_path(cwd.as_ref());
        let mut fs = MemoryFs::default();
        ensure_memory_dir(&mut fs.dirs, &cwd);
        Self {
            cwd,
            fs: Arc::new(Mutex::new(fs)),
        }
    }
}

impl Default for MemorySessionEnv {
    fn default() -> Self {
        Self::new("/workspace")
    }
}

impl SessionEnv for MemorySessionEnv {
    fn exec(
        &self,
        _command: &str,
        _options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        Err(AgenticHarnessError::Handler {
            message:
                "MemorySessionEnv does not provide shell execution; bind a local or remote SessionEnv"
                    .to_string(),
        })
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> {
        let path = self.resolve_path(path);
        let fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        let content = fs
            .files
            .get(&path)
            .ok_or_else(|| AgenticHarnessError::Handler {
                message: format!("memory file {} was not found", path.display()),
            })?;
        String::from_utf8(content.clone()).map_err(|err| AgenticHarnessError::Handler {
            message: format!("memory file {} is not valid UTF-8: {err}", path.display()),
        })
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> {
        let path = self.resolve_path(path);
        let mut fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        if let Some(parent) = path.parent() {
            ensure_memory_dir(&mut fs.dirs, parent);
        }
        fs.files.insert(path, content.to_vec());
        Ok(())
    }

    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        let path = self.resolve_path(path);
        let fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        if let Some(content) = fs.files.get(&path) {
            return Ok(FileStat {
                is_file: true,
                is_directory: false,
                is_symbolic_link: false,
                size: content.len() as u64,
                modified_unix_ms: None,
            });
        }
        if fs.dirs.contains(&path) {
            return Ok(FileStat {
                is_file: false,
                is_directory: true,
                is_symbolic_link: false,
                size: 0,
                modified_unix_ms: None,
            });
        }
        Err(AgenticHarnessError::Handler {
            message: format!("memory path {} was not found", path.display()),
        })
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        let path = self.resolve_path(path);
        let fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        if !fs.dirs.contains(&path) {
            return Err(AgenticHarnessError::Handler {
                message: format!("memory directory {} was not found", path.display()),
            });
        }
        let mut entries = BTreeSet::new();
        for file in fs.files.keys() {
            if file.parent() == Some(path.as_path()) {
                if let Some(name) = file.file_name().and_then(|name| name.to_str()) {
                    entries.insert(name.to_string());
                }
            }
        }
        for dir in &fs.dirs {
            if dir != &path && dir.parent() == Some(path.as_path()) {
                if let Some(name) = dir.file_name().and_then(|name| name.to_str()) {
                    entries.insert(name.to_string());
                }
            }
        }
        Ok(entries.into_iter().collect())
    }

    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        let path = self.resolve_path(path);
        let fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        Ok(fs.files.contains_key(&path) || fs.dirs.contains(&path))
    }

    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        let path = self.resolve_path(path);
        let mut fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        ensure_memory_dir(&mut fs.dirs, &path);
        Ok(())
    }

    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        let path = self.resolve_path(path);
        let mut fs = self.fs.lock().map_err(|_| AgenticHarnessError::Handler {
            message: "memory session env lock poisoned".to_string(),
        })?;
        if fs.files.remove(&path).is_some() {
            return Ok(());
        }
        if !fs.dirs.contains(&path) {
            return Err(AgenticHarnessError::Handler {
                message: format!("memory path {} was not found", path.display()),
            });
        }
        let has_children = fs
            .files
            .keys()
            .any(|file| file.parent() == Some(path.as_path()))
            || fs
                .dirs
                .iter()
                .any(|dir| dir != &path && dir.parent() == Some(path.as_path()));
        if has_children && !recursive {
            return Err(AgenticHarnessError::Handler {
                message: format!("memory directory {} is not empty", path.display()),
            });
        }
        fs.files.retain(|file, _| !file.starts_with(&path));
        fs.dirs
            .retain(|dir| dir == Path::new("/") || !dir.starts_with(&path));
        Ok(())
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            normalize_memory_path(path)
        } else {
            normalize_memory_path(&self.cwd.join(path))
        }
    }
}

/// Hostless virtual session environment with an in-memory filesystem and a
/// small built-in command set.
#[derive(Debug, Clone)]
pub struct VirtualSessionEnv {
    inner: MemorySessionEnv,
}

impl VirtualSessionEnv {
    pub fn new(cwd: impl AsRef<Path>) -> Self {
        Self {
            inner: MemorySessionEnv::new(cwd),
        }
    }

    pub fn with_file(
        self,
        path: impl AsRef<str>,
        content: impl AsRef<[u8]>,
    ) -> Result<Self, AgenticHarnessError> {
        self.inner.write_file(path.as_ref(), content.as_ref())?;
        Ok(self)
    }

    fn scoped(&self, options: &ShellOptions) -> MemorySessionEnv {
        let cwd = options
            .cwd
            .as_deref()
            .map(|cwd| {
                if cwd.is_absolute() {
                    normalize_memory_path(cwd)
                } else {
                    normalize_memory_path(&self.inner.cwd.join(cwd))
                }
            })
            .unwrap_or_else(|| self.inner.cwd.clone());
        MemorySessionEnv {
            cwd,
            fs: self.inner.fs.clone(),
        }
    }

    fn unsupported(command: &str) -> ShellOutput {
        ShellOutput {
            stdout: String::new(),
            stderr: format!(
                "virtual shell command is not supported: {command}\nSupported commands: pwd, ls, cat, echo >, mkdir, rm, grep\n"
            ),
            exit_code: 127,
        }
    }
}

impl Default for VirtualSessionEnv {
    fn default() -> Self {
        Self::new("/workspace")
    }
}

impl SessionEnv for VirtualSessionEnv {
    fn exec(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        let scoped = self.scoped(&options);
        let command = command.trim();
        if command.is_empty() {
            return Ok(ShellOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        if command == "pwd" {
            return Ok(ShellOutput {
                stdout: format!("{}\n", scoped.cwd().display()),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        if let Some(rest) = command.strip_prefix("cat ") {
            return Ok(match scoped.read_file(rest.trim()) {
                Ok(stdout) => ShellOutput {
                    stdout,
                    stderr: String::new(),
                    exit_code: 0,
                },
                Err(err) => ShellOutput {
                    stdout: String::new(),
                    stderr: format!("{err}\n"),
                    exit_code: 1,
                },
            });
        }
        if let Some(rest) = command.strip_prefix("ls") {
            let path = rest.trim();
            let entries = scoped.readdir(if path.is_empty() { "." } else { path })?;
            let mut stdout = entries.join("\n");
            if !stdout.is_empty() {
                stdout.push('\n');
            }
            return Ok(ShellOutput {
                stdout,
                stderr: String::new(),
                exit_code: 0,
            });
        }
        if let Some((left, path)) = command.split_once('>') {
            if let Some(text) = left.trim().strip_prefix("echo ") {
                let text = text.trim().trim_matches('"').trim_matches('\'');
                scoped.write_file(path.trim(), format!("{text}\n").as_bytes())?;
                return Ok(ShellOutput {
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: 0,
                });
            }
        }
        if let Some(rest) = command.strip_prefix("mkdir ") {
            for path in rest.split_whitespace().filter(|part| *part != "-p") {
                scoped.mkdir(path)?;
            }
            return Ok(ShellOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        if let Some(rest) = command.strip_prefix("rm ") {
            let mut recursive = false;
            for part in rest.split_whitespace() {
                if part.starts_with('-') {
                    recursive |= part.contains('r');
                    continue;
                }
                scoped.rm(part, recursive)?;
            }
            return Ok(ShellOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
            });
        }
        if let Some(rest) = command.strip_prefix("grep ") {
            let mut parts = rest.split_whitespace();
            let needle = parts.next().unwrap_or_default();
            let path = parts.next().unwrap_or_default();
            if needle.is_empty() || path.is_empty() {
                return Ok(Self::unsupported(command));
            }
            let content = scoped.read_file(path)?;
            let matches = content
                .lines()
                .filter(|line| line.contains(needle))
                .collect::<Vec<_>>();
            let mut stdout = matches.join("\n");
            if !stdout.is_empty() {
                stdout.push('\n');
            }
            let exit_code = if stdout.is_empty() { 1 } else { 0 };
            return Ok(ShellOutput {
                stdout,
                stderr: String::new(),
                exit_code,
            });
        }
        Ok(Self::unsupported(command))
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> {
        self.inner.read_file(path)
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> {
        self.inner.write_file(path, content)
    }

    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        self.inner.stat(path)
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        self.inner.readdir(path)
    }

    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        self.inner.exists(path)
    }

    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        self.inner.mkdir(path)
    }

    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        self.inner.rm(path, recursive)
    }

    fn cwd(&self) -> &Path {
        self.inner.cwd()
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        self.inner.resolve_path(path)
    }
}

/// Raw response returned by an [`HttpSessionTransport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpSessionTransportResponse {
    pub status: u16,
    pub body: String,
}

/// Transport used by [`HttpSessionEnv`] to exchange JSON with a remote sandbox.
pub trait HttpSessionTransport: Send + Sync {
    fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
    ) -> Result<HttpSessionTransportResponse, AgenticHarnessError>;
}

#[cfg(feature = "native")]
#[derive(Debug, Clone)]
struct ReqwestSessionTransport {
    client: reqwest::blocking::Client,
}

#[cfg(feature = "native")]
impl Default for ReqwestSessionTransport {
    fn default() -> Self {
        Self {
            client: reqwest::blocking::Client::new(),
        }
    }
}

#[cfg(feature = "native")]
impl HttpSessionTransport for ReqwestSessionTransport {
    fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
    ) -> Result<HttpSessionTransportResponse, AgenticHarnessError> {
        let mut request = self.client.post(url).json(body);
        for (name, value) in headers {
            request = request.header(name, value);
        }
        let response = request.send()?;
        let status = response.status().as_u16();
        let body = response.text()?;
        Ok(HttpSessionTransportResponse { status, body })
    }
}

/// HTTP-backed session environment for remote sandbox connectors.
///
/// The remote endpoint receives JSON `POST` requests with an `op` field. This
/// keeps provider-specific sandbox code outside Agentic Harness while giving
/// native Rust agents a reusable adapter for remote command and file helpers.
#[derive(Clone)]
pub struct HttpSessionEnv {
    base_url: String,
    cwd: PathBuf,
    headers: BTreeMap<String, String>,
    transport: Arc<dyn HttpSessionTransport>,
}

impl HttpSessionEnv {
    #[cfg(feature = "native")]
    pub fn new(base_url: impl Into<String>, cwd: impl AsRef<Path>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            cwd: cwd.as_ref().to_path_buf(),
            headers: BTreeMap::new(),
            transport: Arc::new(ReqwestSessionTransport::default()),
        }
    }

    pub fn with_transport<T: HttpSessionTransport + 'static>(
        base_url: impl Into<String>,
        cwd: impl AsRef<Path>,
        transport: T,
    ) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            cwd: cwd.as_ref().to_path_buf(),
            headers: BTreeMap::new(),
            transport: Arc::new(transport),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn transport<T: HttpSessionTransport + 'static>(mut self, transport: T) -> Self {
        self.transport = Arc::new(transport);
        self
    }

    fn request<T: DeserializeOwned>(
        &self,
        op: &str,
        body: Value,
    ) -> Result<T, AgenticHarnessError> {
        let response = self
            .transport
            .post_json(&self.base_url, &self.headers, &body)?;
        if !(200..300).contains(&response.status) {
            let message = remote_error_message(&response.body);
            return Err(AgenticHarnessError::Handler {
                message: format!("remote {op} failed: {message}"),
            });
        }

        serde_json::from_str(&response.body).map_err(AgenticHarnessError::from)
    }

    fn acknowledge(&self, op: &str, body: Value) -> Result<(), AgenticHarnessError> {
        let _: Value = self.request(op, body)?;
        Ok(())
    }
}

impl std::fmt::Debug for HttpSessionEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HttpSessionEnv")
            .field("base_url", &self.base_url)
            .field("cwd", &self.cwd)
            .field("headers", &self.headers.keys().collect::<Vec<_>>())
            .finish_non_exhaustive()
    }
}

impl SessionEnv for HttpSessionEnv {
    fn exec(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        let cwd = options.cwd.as_ref().unwrap_or(&self.cwd);
        let mut body = json!({
            "op": "exec",
            "command": command,
            "cwd": cwd.to_string_lossy(),
            "env": options.env
        });
        if let Some(timeout) = options.timeout {
            body["timeoutMs"] = json!(timeout.as_millis() as u64);
        }
        self.request("exec", body)
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> {
        self.request("read", json!({ "op": "read", "path": path }))
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> {
        let mut body = json!({ "op": "write", "path": path });
        if let Ok(text) = std::str::from_utf8(content) {
            body["content"] = json!(text);
        } else {
            body["contentBase64"] = json!(base64_encode(content));
        }
        self.acknowledge("write", body)
    }

    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        self.request("stat", json!({ "op": "stat", "path": path }))
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        self.request("readdir", json!({ "op": "readdir", "path": path }))
    }

    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        self.request("exists", json!({ "op": "exists", "path": path }))
    }

    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        self.acknowledge("mkdir", json!({ "op": "mkdir", "path": path }))
    }

    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        self.acknowledge(
            "rm",
            json!({ "op": "rm", "path": path, "recursive": recursive }),
        )
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }
}

/// Known hosted sandbox providers for first-class `HttpSessionEnv` bridges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SandboxProvider {
    Vercel,
    Daytona,
    E2b,
    Custom(String),
}

impl SandboxProvider {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Vercel => "vercel",
            Self::Daytona => "daytona",
            Self::E2b => "e2b",
            Self::Custom(name) => name,
        }
    }
}

/// Provider-scoped HTTP sandbox connector for hosted coding environments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxConnector {
    provider: SandboxProvider,
    endpoint: String,
    cwd: PathBuf,
    headers: BTreeMap<String, String>,
}

impl SandboxConnector {
    pub fn vercel(endpoint: impl Into<String>, cwd: impl AsRef<Path>) -> Self {
        Self::new(SandboxProvider::Vercel, endpoint, cwd)
    }

    pub fn daytona(endpoint: impl Into<String>, cwd: impl AsRef<Path>) -> Self {
        Self::new(SandboxProvider::Daytona, endpoint, cwd)
    }

    pub fn e2b(endpoint: impl Into<String>, cwd: impl AsRef<Path>) -> Self {
        Self::new(SandboxProvider::E2b, endpoint, cwd)
    }

    pub fn custom(
        provider: impl Into<String>,
        endpoint: impl Into<String>,
        cwd: impl AsRef<Path>,
    ) -> Self {
        Self::new(SandboxProvider::Custom(provider.into()), endpoint, cwd)
    }

    pub fn new(
        provider: SandboxProvider,
        endpoint: impl Into<String>,
        cwd: impl AsRef<Path>,
    ) -> Self {
        Self {
            provider,
            endpoint: endpoint.into(),
            cwd: cwd.as_ref().to_path_buf(),
            headers: BTreeMap::new(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), value.into());
        self
    }

    pub fn provider(&self) -> &SandboxProvider {
        &self.provider
    }

    #[cfg(feature = "native")]
    pub fn into_session_env(self) -> HttpSessionEnv {
        self.apply_headers(HttpSessionEnv::new(self.endpoint.clone(), &self.cwd))
    }

    pub fn into_session_env_with_transport<T: HttpSessionTransport + 'static>(
        self,
        transport: T,
    ) -> HttpSessionEnv {
        let provider = self.provider.as_str().to_string();
        let transport = ProviderTaggedSessionTransport {
            provider,
            inner: transport,
        };
        self.apply_headers(HttpSessionEnv::with_transport(
            self.endpoint.clone(),
            &self.cwd,
            transport,
        ))
    }

    fn apply_headers(&self, mut env: HttpSessionEnv) -> HttpSessionEnv {
        env = env.header("x-agentic-harness-sandbox-provider", self.provider.as_str());
        for (name, value) in &self.headers {
            env = env.header(name, value);
        }
        env
    }
}

#[derive(Debug, Clone)]
struct ProviderTaggedSessionTransport<T> {
    provider: String,
    inner: T,
}

impl<T: HttpSessionTransport> HttpSessionTransport for ProviderTaggedSessionTransport<T> {
    fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
    ) -> Result<HttpSessionTransportResponse, AgenticHarnessError> {
        let mut body = body.clone();
        if let Value::Object(object) = &mut body {
            object.insert("provider".to_string(), json!(self.provider));
        }
        self.inner.post_json(url, headers, &body)
    }
}

fn remote_error_message(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .or_else(|| {
                    value
                        .get("message")
                        .and_then(Value::as_str)
                        .map(ToOwned::to_owned)
                })
        })
        .filter(|message| !message.trim().is_empty())
        .unwrap_or_else(|| body.trim().to_string())
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        output.push(ALPHABET[(b0 >> 2) as usize] as char);
        output.push(ALPHABET[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(b2 & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn normalize_memory_path(path: &Path) -> PathBuf {
    let input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        PathBuf::from("/").join(path)
    };
    let mut normalized = PathBuf::from("/");
    for component in input.components() {
        match component {
            std::path::Component::RootDir => {
                normalized = PathBuf::from("/");
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
                if normalized.as_os_str().is_empty() {
                    normalized = PathBuf::from("/");
                }
            }
            std::path::Component::Normal(part) => normalized.push(part),
            std::path::Component::Prefix(_) => {}
        }
    }
    normalized
}

fn ensure_memory_dir(dirs: &mut BTreeSet<PathBuf>, path: &Path) {
    let path = normalize_memory_path(path);
    let mut current = PathBuf::from("/");
    dirs.insert(current.clone());
    for component in path.components() {
        if let std::path::Component::Normal(part) = component {
            current.push(part);
            dirs.insert(current.clone());
        }
    }
}

/// Per-invocation session with prompt, skill, shell, and history helpers.
pub struct Session {
    context: AgentContext,
    env: SharedSessionEnv,
    key: String,
    data: SessionData,
    history: Vec<ModelMessage>,
}

impl Session {
    pub fn prompt<M: ModelClient + ?Sized>(
        &mut self,
        text: impl Into<String>,
        model: &M,
    ) -> Result<PromptResponse, AgenticHarnessError> {
        self.prompt_inner(text, model, None, Vec::new(), None, None)
    }

    pub fn prompt_with_options(
        &mut self,
        text: impl Into<String>,
        options: PromptOptions,
    ) -> Result<PromptResponse, AgenticHarnessError> {
        let tools = self.context.resolve_tool_defs(&options.tools)?;
        let model = self.context.resolve_model(&options)?;
        self.prompt_inner(
            text,
            model.as_ref(),
            options.role.as_deref(),
            tools,
            options.compaction.as_ref(),
            options.result_schema.as_ref(),
        )
    }

    fn prompt_inner<M: ModelClient + ?Sized>(
        &mut self,
        text: impl Into<String>,
        model: &M,
        role: Option<&str>,
        tools: Vec<ToolDef>,
        compaction: Option<&CompactionSettings>,
        result_schema: Option<&Value>,
    ) -> Result<PromptResponse, AgenticHarnessError> {
        let content = build_prompt_text(text.into(), result_schema)?;
        let user = ModelMessage {
            role: "user".to_string(),
            content,
        };
        self.data.append_message(user, Some("user".to_string()));
        self.refresh_history();
        if let Some(settings) = compaction {
            self.maybe_auto_compact(model, settings)?;
        }
        let system = self.system_prompt(role)?;
        let mut tool_specs = tools
            .iter()
            .map(|tool| tool.spec().clone())
            .collect::<Vec<_>>();
        tool_specs.extend(builtin_tool_specs());

        for _ in 0..MAX_TOOL_CALL_ROUNDS {
            let request = ModelRequest {
                system: system.clone(),
                messages: self.history.clone(),
                tools: tool_specs.clone(),
            };
            let response = model.complete(request)?;

            if response.tool_calls.is_empty() {
                if !response.text.is_empty() {
                    self.context
                        .emit_event("text_delta", json!({ "text": response.text }));
                }
                self.data.append_message(
                    ModelMessage {
                        role: "assistant".to_string(),
                        content: response.text.clone(),
                    },
                    Some("assistant".to_string()),
                );
                self.refresh_history();
                self.context.save_session_data(&self.key, &self.data)?;
                self.context.emit_event("turn_end", json!({}));
                return Ok(response);
            }

            self.data.append_message(
                ModelMessage {
                    role: "assistant".to_string(),
                    content: assistant_tool_call_history(&response),
                },
                Some("assistant".to_string()),
            );
            self.refresh_history();
            for tool_call in &response.tool_calls {
                self.context.emit_event(
                    "tool_start",
                    json!({
                        "toolName": tool_call.name,
                        "toolCallId": tool_call.id,
                        "args": tool_call.arguments
                    }),
                );
                let result = match self.execute_model_tool_call(model, &tools, tool_call) {
                    Ok(result) => result,
                    Err(err) => {
                        self.context.emit_event(
                            "tool_end",
                            json!({
                                "toolName": tool_call.name,
                                "toolCallId": tool_call.id,
                                "isError": true,
                                "result": err.to_string()
                            }),
                        );
                        return Err(err);
                    }
                };
                self.context.emit_event(
                    "tool_end",
                    json!({
                        "toolName": tool_call.name,
                        "toolCallId": tool_call.id,
                        "isError": false,
                        "result": result
                    }),
                );
                self.data.append_message(
                    ModelMessage {
                        role: "tool".to_string(),
                        content: serde_json::to_string(&json!({
                            "toolCallId": tool_call.id,
                            "name": tool_call.name,
                            "result": result
                        }))?,
                    },
                    Some("tool".to_string()),
                );
                self.refresh_history();
            }
        }

        Err(AgenticHarnessError::Handler {
            message: format!("model requested more than {MAX_TOOL_CALL_ROUNDS} tool call rounds"),
        })
    }

    fn execute_model_tool_call<M: ModelClient + ?Sized>(
        &self,
        model: &M,
        tools: &[ToolDef],
        tool_call: &ToolCall,
    ) -> Result<String, AgenticHarnessError> {
        if BUILTIN_TOOL_NAMES.contains(&tool_call.name.as_str()) {
            return self.execute_builtin_tool_call(model, tool_call);
        }
        execute_custom_model_tool_call(tools, tool_call)
    }

    fn execute_builtin_tool_call<M: ModelClient + ?Sized>(
        &self,
        model: &M,
        tool_call: &ToolCall,
    ) -> Result<String, AgenticHarnessError> {
        let args = &tool_call.arguments;
        match tool_call.name.as_str() {
            "read" => {
                let path = required_string_arg(args, "path", "read")?;
                let mut options = ReadOptions::new();
                if let Some(offset) = optional_usize_arg(args, "offset", "read")? {
                    options = options.offset(offset);
                }
                if let Some(max_bytes) = optional_usize_arg(args, "maxBytes", "read")? {
                    options = options.max_bytes(max_bytes);
                }
                serde_json::to_string(&self.read_with_options(&path, options)?)
                    .map_err(AgenticHarnessError::from)
            }
            "write" => {
                let path = required_string_arg(args, "path", "write")?;
                let content = required_string_arg(args, "content", "write")?;
                self.write(&path, content)?;
                Ok(format!("Wrote {path}"))
            }
            "edit" => {
                let path = required_string_arg(args, "path", "edit")?;
                let old = required_string_arg(args, "oldText", "edit")?;
                let new = required_string_arg(args, "newText", "edit")?;
                self.edit(&path, &old, &new)?;
                Ok(format!("Edited {path}"))
            }
            "bash" => {
                let command = required_string_arg(args, "command", "bash")?;
                serde_json::to_string(
                    &self.shell_with_options(&command, shell_options_from_tool_args(args)?)?,
                )
                .map_err(AgenticHarnessError::from)
            }
            "grep" => {
                let pattern = required_string_arg(args, "pattern", "grep")?;
                serde_json::to_string(&self.grep(&pattern)?).map_err(AgenticHarnessError::from)
            }
            "glob" => {
                let pattern = required_string_arg(args, "pattern", "glob")?;
                serde_json::to_string(&self.glob(&pattern)?).map_err(AgenticHarnessError::from)
            }
            "task" => {
                let prompt = required_string_arg(args, "prompt", "task")?;
                let id = args
                    .get("id")
                    .and_then(Value::as_str)
                    .map(ToString::to_string)
                    .unwrap_or_else(|| self.generated_task_id(&prompt));
                Ok(self.task_with_id(id, prompt, model)?.text)
            }
            _ => Err(AgenticHarnessError::ToolNotFound {
                name: tool_call.name.clone(),
                available: BUILTIN_TOOL_NAMES
                    .iter()
                    .map(|name| name.to_string())
                    .collect(),
            }),
        }
    }

    fn maybe_auto_compact<M: ModelClient + ?Sized>(
        &mut self,
        model: &M,
        settings: &CompactionSettings,
    ) -> Result<(), AgenticHarnessError> {
        if !settings.enabled {
            return Ok(());
        }

        let budget = settings
            .context_window_tokens
            .saturating_sub(settings.reserve_tokens)
            .max(1);
        let tokens_before = estimate_context_tokens(&self.history);
        if tokens_before <= budget {
            return Ok(());
        }

        let transcript = self.compaction_transcript(settings.keep_recent_messages);
        if transcript.trim().is_empty() {
            return Ok(());
        }

        let summary_response = model.complete(ModelRequest {
            system: automatic_compaction_system_prompt().to_string(),
            messages: vec![ModelMessage {
                role: "user".to_string(),
                content: transcript,
            }],
            tools: Vec::new(),
        })?;
        if !summary_response.tool_calls.is_empty() {
            return Err(AgenticHarnessError::Handler {
                message: "compaction model returned tool calls; expected a text summary"
                    .to_string(),
            });
        }
        let summary = summary_response.text.trim();
        if summary.is_empty() {
            return Err(AgenticHarnessError::Handler {
                message: "compaction model returned an empty summary".to_string(),
            });
        }

        self.data.append_compaction(
            summary.to_string(),
            CompactionOptions::new()
                .keep_recent_messages(settings.keep_recent_messages)
                .tokens_before(tokens_before),
        )?;
        self.refresh_history();
        self.context.save_session_data(&self.key, &self.data)?;
        self.context.emit_event(
            "compaction",
            json!({
                "tokensBefore": tokens_before,
                "contextWindowTokens": settings.context_window_tokens,
                "reserveTokens": settings.reserve_tokens,
                "keepRecentMessages": settings.keep_recent_messages
            }),
        );
        Ok(())
    }

    pub fn prompt_json<T: DeserializeOwned, M: ModelClient + ?Sized>(
        &mut self,
        text: impl Into<String>,
        model: &M,
    ) -> Result<T, AgenticHarnessError> {
        self.prompt(text, model)?.result_json()
    }

    pub fn prompt_json_with_options<T: DeserializeOwned>(
        &mut self,
        text: impl Into<String>,
        options: PromptOptions,
    ) -> Result<T, AgenticHarnessError> {
        self.prompt_with_options(text, options)?.result_json()
    }

    pub fn skill<M: ModelClient + ?Sized>(
        &mut self,
        name: &str,
        args: Value,
        model: &M,
    ) -> Result<PromptResponse, AgenticHarnessError> {
        let skill = self.context.require_skill(name)?;
        let args = serde_json::to_string_pretty(&args)?;
        self.prompt(
            format!("{}\n\nArguments:\n{}", skill.instructions.trim(), args),
            model,
        )
    }

    pub fn skill_json<T: DeserializeOwned, M: ModelClient + ?Sized>(
        &mut self,
        name: &str,
        args: Value,
        model: &M,
    ) -> Result<T, AgenticHarnessError> {
        self.skill(name, args, model)?.result_json()
    }

    pub fn task<M: ModelClient + ?Sized>(
        &self,
        text: impl Into<String>,
        model: &M,
    ) -> Result<PromptResponse, AgenticHarnessError> {
        let text = text.into();
        let id = self.generated_task_id(&text);
        self.task_with_id(id, text, model)
    }

    pub fn task_with_id<M: ModelClient + ?Sized>(
        &self,
        task_id: impl AsRef<str>,
        text: impl Into<String>,
        model: &M,
    ) -> Result<PromptResponse, AgenticHarnessError> {
        let mut task = self
            .context
            .session_with_shared_env(&format!("task:{}", task_id.as_ref()), self.env.clone());
        task.prompt(text, model)
    }

    pub fn task_json<T: DeserializeOwned, M: ModelClient + ?Sized>(
        &self,
        task_id: impl AsRef<str>,
        text: impl Into<String>,
        model: &M,
    ) -> Result<T, AgenticHarnessError> {
        self.task_with_id(task_id, text, model)?.result_json()
    }

    pub fn compact_with_summary(
        &mut self,
        summary: impl Into<String>,
        options: CompactionOptions,
    ) -> Result<(), AgenticHarnessError> {
        self.data.append_compaction(summary.into(), options)?;
        self.refresh_history();
        self.context.save_session_data(&self.key, &self.data)
    }

    pub fn append_branch_summary(
        &mut self,
        summary: impl Into<String>,
        from_id: impl Into<String>,
        details: Value,
    ) -> Result<(), AgenticHarnessError> {
        self.data
            .append_branch_summary(summary.into(), from_id.into(), Some(details));
        self.refresh_history();
        self.context.save_session_data(&self.key, &self.data)
    }

    pub fn shell(&self, command: &str) -> Result<ShellOutput, AgenticHarnessError> {
        self.env.exec(command, ShellOptions::default())
    }

    pub fn shell_with_options(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        self.env.exec(command, options)
    }

    pub fn read(&self, path: &str) -> Result<String, AgenticHarnessError> {
        self.env.read_file(path)
    }

    pub fn read_with_options(
        &self,
        path: &str,
        options: ReadOptions,
    ) -> Result<ReadOutput, AgenticHarnessError> {
        bounded_read(&self.env.read_file(path)?, options)
    }

    pub fn write(&self, path: &str, content: impl AsRef<[u8]>) -> Result<(), AgenticHarnessError> {
        self.env.write_file(path, content.as_ref())
    }

    pub fn edit(&self, path: &str, old: &str, new: &str) -> Result<(), AgenticHarnessError> {
        let content = self.env.read_file(path)?;
        let occurrences = content.matches(old).count();
        if occurrences == 0 {
            return Err(AgenticHarnessError::Handler {
                message: format!("Could not find exact text in {path}."),
            });
        }
        if occurrences > 1 {
            return Err(AgenticHarnessError::Handler {
                message: format!(
                    "Found {occurrences} occurrences in {path}; provide a unique old text."
                ),
            });
        }
        self.env
            .write_file(path, content.replacen(old, new, 1).as_bytes())
    }

    pub fn grep(&self, pattern: &str) -> Result<Vec<GrepMatch>, AgenticHarnessError> {
        grep_env(self.env.as_ref(), pattern)
    }

    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, AgenticHarnessError> {
        glob_env(self.env.as_ref(), pattern)
    }

    pub fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        self.env.stat(path)
    }

    pub fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        self.env.readdir(path)
    }

    pub fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        self.env.exists(path)
    }

    pub fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        self.env.mkdir(path)
    }

    pub fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        self.env.rm(path, recursive)
    }

    pub fn history(&self) -> &[ModelMessage] {
        &self.history
    }

    pub fn session_data(&self) -> &SessionData {
        &self.data
    }

    fn system_prompt(&self, role: Option<&str>) -> Result<String, AgenticHarnessError> {
        let mut prompt = compose_system_prompt(&self.context.workspace, &self.context.skills)?;
        if let Some(role_name) = role {
            let role = self.context.require_role(role_name)?;
            prompt.push_str("\n\n## Role: ");
            prompt.push_str(&role.name);
            if !role.description.is_empty() {
                prompt.push('\n');
                prompt.push_str(&role.description);
            }
            if !role.instructions.trim().is_empty() {
                prompt.push_str("\n\n");
                prompt.push_str(role.instructions.trim());
            }
        }
        Ok(prompt)
    }

    fn generated_task_id(&self, text: &str) -> String {
        let mut hasher = DefaultHasher::new();
        self.key.hash(&mut hasher);
        self.history.len().hash(&mut hasher);
        text.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn compaction_transcript(&self, keep_recent_messages: usize) -> String {
        let path = self.data.active_entries();
        let message_positions = path
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.is_message() || entry.is_branch_summary())
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let keep = keep_recent_messages.min(message_positions.len());
        let compact_end = if keep == 0 {
            path.len()
        } else {
            message_positions[message_positions.len() - keep]
        };
        serialize_messages_for_compaction(&entries_to_compaction_context(&path[..compact_end]))
    }

    fn refresh_history(&mut self) {
        self.history = self.data.context_messages();
    }
}

/// Trigger metadata for an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triggers {
    pub webhook: bool,
}

/// Serializable manifest returned by `/agents`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub agents: Vec<ManifestAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestAgent {
    pub name: String,
    pub triggers: Triggers,
}

/// Cloudflare Worker deployment metadata derived from registered agents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareWorkerManifest {
    pub durable_objects: Vec<CloudflareDurableObjectBinding>,
    pub webhook_agent_names: Vec<String>,
}

impl CloudflareWorkerManifest {
    pub fn wrangler_config(&self, name: impl Into<String>, main: impl Into<String>) -> Value {
        let bindings = self
            .durable_objects
            .iter()
            .map(|binding| {
                json!({
                    "name": binding.binding_name,
                    "class_name": binding.class_name
                })
            })
            .collect::<Vec<_>>();
        let migrations = self
            .durable_objects
            .iter()
            .map(|binding| {
                json!({
                    "tag": format!("agentic-harness-{}", binding.class_name),
                    "new_sqlite_classes": [binding.class_name.clone()]
                })
            })
            .collect::<Vec<_>>();

        json!({
            "$schema": "https://workers.cloudflare.com/schema/wrangler.json",
            "name": name.into(),
            "main": main.into(),
            "compatibility_date": CLOUDFLARE_COMPATIBILITY_DATE,
            "durable_objects": {
                "bindings": bindings
            },
            "migrations": migrations
        })
    }

    pub fn worker_entrypoint(&self, worker_module: impl AsRef<str>) -> String {
        let module = worker_module.as_ref();
        let webhook_agent_names =
            serde_json::to_string(&self.webhook_agent_names).unwrap_or_else(|_| "[]".to_string());
        let bindings = self
            .durable_objects
            .iter()
            .map(|binding| (binding.agent_name.clone(), binding.binding_name.clone()))
            .collect::<BTreeMap<_, _>>();
        let bindings = serde_json::to_string(&bindings).unwrap_or_else(|_| "{}".to_string());
        let manifest = serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string());
        let classes = self
            .durable_objects
            .iter()
            .map(|binding| {
                let class_name = &binding.class_name;
                let agent_name = serde_json::to_string(&binding.agent_name)
                    .unwrap_or_else(|_| "\"\"".to_string());
                format!(
                    "export class {class_name} {{\n  static agentName = {agent_name};\n\n  constructor(state, env) {{\n    this.state = state;\n    this.env = env;\n  }}\n\n  async fetch(request) {{\n    await initWorker();\n    return handleAgenticHarnessDurableObject(request, this.state, this.env, {agent_name});\n  }}\n}}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n");

        format!(
            r#"// Auto-generated by Agentic Harness Cloudflare build.
import initWorker, {{ handleAgenticHarnessDurableObject }} from '{module}';

const MANIFEST = {manifest};
const WEBHOOK_AGENT_NAMES = new Set({webhook_agent_names});
const DURABLE_OBJECT_BINDINGS = {bindings};

function jsonResponse(value, status = 200) {{
  return new Response(JSON.stringify(value), {{
    status,
    headers: {{ 'content-type': 'application/json' }},
  }});
}}

{classes}

export default {{
  async fetch(request, env) {{
    const url = new URL(request.url);
    if (request.method === 'GET' && url.pathname === '/health') {{
      return jsonResponse({{ status: 'ok' }});
    }}
    if (request.method === 'GET' && url.pathname === '/agents') {{
      return jsonResponse(MANIFEST);
    }}

    const match = url.pathname.match(/^\/agents\/([^/]+)\/([^/]+)\/?$/);
    if (!match) {{
      return jsonResponse({{ error: {{ type: 'route_not_found', message: `No route for ${{request.method}} ${{url.pathname}}.` }} }}, 404);
    }}
    if (request.method !== 'POST') {{
      return jsonResponse({{ error: {{ type: 'method_not_allowed', message: `Method "${{request.method}}" is not allowed for this route.` }} }}, 405);
    }}

    const agentName = decodeURIComponent(match[1]);
    const id = decodeURIComponent(match[2]);
    if (!WEBHOOK_AGENT_NAMES.has(agentName)) {{
      return jsonResponse({{ error: {{ type: 'agent_not_found', message: `Agent "${{agentName}}" was not found.` }} }}, 404);
    }}

    const bindingName = DURABLE_OBJECT_BINDINGS[agentName];
    const namespace = env[bindingName];
    if (!namespace) {{
      return jsonResponse({{ error: {{ type: 'deployment_error', message: `Missing Durable Object binding "${{bindingName}}".` }} }}, 500);
    }}
    const objectId = namespace.idFromName(id);
    const stub = namespace.get(objectId);
    return stub.fetch(request);
  }},
}};
"#
        )
    }
}

/// Durable Object class and binding required for one webhook agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareDurableObjectBinding {
    pub agent_name: String,
    pub binding_name: String,
    pub class_name: String,
}

/// HTTP response produced by [`AgentApp::handle_http`] or [`AgentApp::handle_request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Platform-neutral HTTP request shape for Worker or native adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl HttpRequest {
    pub fn new(method: impl Into<String>, path: impl Into<String>, body: impl AsRef<[u8]>) -> Self {
        Self {
            method: method.into(),
            path: path.into(),
            headers: Vec::new(),
            body: body.as_ref().to_vec(),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    fn header_value(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .rev()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// JSON request shape used by Worker/WASM app adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAppRequest {
    pub agent_name: String,
    pub id: String,
    pub payload: Value,
}

impl WorkerAppRequest {
    pub fn new(
        agent_name: impl Into<String>,
        id: impl Into<String>,
        payload: impl Into<Value>,
    ) -> Self {
        Self {
            agent_name: agent_name.into(),
            id: id.into(),
            payload: payload.into(),
        }
    }
}

/// JSON response shape returned by Worker/WASM app adapters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerAppResponse {
    pub status: u16,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub events: Vec<RuntimeEvent>,
}

impl WorkerAppResponse {
    pub fn ok(result: Value, events: Vec<RuntimeEvent>) -> Self {
        Self {
            status: 200,
            result: Some(result),
            error: None,
            events,
        }
    }

    pub fn from_error(error: AgenticHarnessError, events: Vec<RuntimeEvent>) -> Self {
        Self {
            status: error.status(),
            result: None,
            error: Some(error.envelope()),
            events,
        }
    }
}

pub trait IntoAgentAppResult {
    fn into_agent_app_result(self) -> Result<AgentApp, AgenticHarnessError>;
}

impl IntoAgentAppResult for AgentApp {
    fn into_agent_app_result(self) -> Result<AgentApp, AgenticHarnessError> {
        Ok(self)
    }
}

impl IntoAgentAppResult for Result<AgentApp, AgenticHarnessError> {
    fn into_agent_app_result(self) -> Result<AgentApp, AgenticHarnessError> {
        self
    }
}

#[macro_export]
macro_rules! agentic_harness_worker_app {
    ($app:path) => {
        static AGENTIC_HARNESS_LAST_RESULT: std::sync::Mutex<Vec<u8>> =
            std::sync::Mutex::new(Vec::new());

        fn agentic_harness_store_response(response: &$crate::WorkerAppResponse) -> *const u8 {
            let bytes = serde_json::to_vec(response).unwrap_or_else(|err| {
                serde_json::to_vec(&$crate::WorkerAppResponse::from_error(
                    $crate::AgenticHarnessError::Json { source: err },
                    Vec::new(),
                ))
                .unwrap_or_else(|_| {
                    b"{\"status\":500,\"result\":null,\"error\":null,\"events\":[]}".to_vec()
                })
            });
            let mut last = AGENTIC_HARNESS_LAST_RESULT
                .lock()
                .expect("Agentic Harness WASM result lock poisoned");
            *last = bytes;
            last.as_ptr()
        }

        #[no_mangle]
        pub extern "C" fn agentic_harness_alloc(len: usize) -> *mut u8 {
            let mut buffer = Vec::<u8>::with_capacity(len);
            let ptr = buffer.as_mut_ptr();
            std::mem::forget(buffer);
            ptr
        }

        #[no_mangle]
        pub unsafe extern "C" fn agentic_harness_dealloc(ptr: *mut u8, len: usize) {
            if !ptr.is_null() && len > 0 {
                drop(Vec::from_raw_parts(ptr, 0, len));
            }
        }

        #[no_mangle]
        pub extern "C" fn agentic_harness_init() {}

        #[no_mangle]
        pub unsafe extern "C" fn agentic_harness_invoke(ptr: *const u8, len: usize) -> *const u8 {
            let input = if ptr.is_null() || len == 0 {
                &[]
            } else {
                std::slice::from_raw_parts(ptr, len)
            };
            let response = match serde_json::from_slice::<$crate::WorkerAppRequest>(input) {
                Ok(request) => match $crate::IntoAgentAppResult::into_agent_app_result($app()) {
                    Ok(app) => app.handle_worker_app_request(request),
                    Err(err) => $crate::WorkerAppResponse::from_error(err, Vec::new()),
                },
                Err(err) => $crate::WorkerAppResponse::from_error(
                    $crate::AgenticHarnessError::InvalidJson {
                        message: err.to_string(),
                    },
                    Vec::new(),
                ),
            };
            agentic_harness_store_response(&response)
        }

        #[no_mangle]
        pub extern "C" fn agentic_harness_last_result_len() -> usize {
            AGENTIC_HARNESS_LAST_RESULT
                .lock()
                .expect("Agentic Harness WASM result lock poisoned")
                .len()
        }
    };
}

impl HttpResponse {
    pub fn json(status: u16, value: Value) -> Self {
        let body = serde_json::to_vec(&value)
            .unwrap_or_else(|_| b"{\"error\":\"serialization\"}".to_vec());
        Self {
            status,
            headers: vec![("content-type".to_string(), "application/json".to_string())],
            body,
        }
    }

    pub fn sse(events: &[(impl AsRef<str>, Value)]) -> Result<Self, AgenticHarnessError> {
        let mut body = String::new();
        for (event, data) in events {
            body.push_str(&RuntimeEvent::new(event.as_ref(), data.clone()).to_sse_frame()?);
        }
        Ok(Self {
            status: 200,
            headers: vec![
                ("content-type".to_string(), "text/event-stream".to_string()),
                ("cache-control".to_string(), "no-cache".to_string()),
                ("connection".to_string(), "keep-alive".to_string()),
            ],
            body: body.into_bytes(),
        })
    }
}

/// Request context passed to a Rust agent handler.
#[derive(Clone)]
pub struct AgentContext {
    id: String,
    payload: Value,
    workspace: PathBuf,
    roles: Arc<BTreeMap<String, Role>>,
    skills: Arc<BTreeMap<String, Skill>>,
    commands: Arc<BTreeMap<String, CommandDef>>,
    tools: Arc<Vec<ToolDef>>,
    models: Arc<BTreeMap<String, SharedModelClient>>,
    default_model: Option<String>,
    providers: ProvidersConfig,
    sessions: SharedSessionStore,
    persisted_sessions: Option<SharedPersistedSessionStore>,
    events: SharedRuntimeEvents,
}

impl AgentContext {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn payload_value(&self) -> &Value {
        &self.payload
    }

    pub fn payload<T: DeserializeOwned>(&self) -> Result<T, AgenticHarnessError> {
        serde_json::from_value(self.payload.clone()).map_err(|err| {
            AgenticHarnessError::InvalidPayload {
                message: err.to_string(),
            }
        })
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    pub fn role(&self, name: &str) -> Option<&Role> {
        self.roles.get(name)
    }

    pub fn require_role(&self, name: &str) -> Result<&Role, AgenticHarnessError> {
        self.role(name)
            .ok_or_else(|| AgenticHarnessError::ContextNotFound {
                kind: "role",
                name: name.to_string(),
            })
    }

    pub fn skill(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn require_skill(&self, name: &str) -> Result<&Skill, AgenticHarnessError> {
        self.skill(name)
            .ok_or_else(|| AgenticHarnessError::ContextNotFound {
                kind: "skill",
                name: name.to_string(),
            })
    }

    pub fn command<I, S>(&self, name: &str, args: I) -> Result<ShellOutput, AgenticHarnessError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let command =
            self.commands
                .get(name)
                .ok_or_else(|| AgenticHarnessError::CommandNotFound {
                    name: name.to_string(),
                    available: self.command_names(),
                })?;
        let args = args
            .into_iter()
            .map(|arg| arg.as_ref().to_string())
            .collect::<Vec<_>>();
        command.execute(&args)
    }

    pub fn tool(&self, name: &str, args: Value) -> Result<String, AgenticHarnessError> {
        validate_tool_defs(&self.tools, &[])?;
        let tool = self
            .tools
            .iter()
            .find(|tool| tool.name() == name)
            .ok_or_else(|| AgenticHarnessError::ToolNotFound {
                name: name.to_string(),
                available: self.tool_names(),
            })?;
        tool.execute(args)
    }

    fn command_names(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }

    fn tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|tool| tool.name().to_string())
            .collect()
    }

    pub fn provider_settings(&self, provider: &str) -> Option<&ProviderSettings> {
        self.providers.get(provider)
    }

    fn emit_event(&self, event: impl Into<String>, data: Value) {
        (self.events)(RuntimeEvent::new(event, data));
    }

    fn resolve_model(
        &self,
        options: &PromptOptions,
    ) -> Result<SharedModelClient, AgenticHarnessError> {
        let role = match options.role.as_deref() {
            Some(role) => Some(self.require_role(role)?),
            None => None,
        };
        let model = options
            .model
            .clone()
            .or_else(|| role.and_then(|role| role.model.clone()))
            .or_else(|| self.default_model.clone())
            .ok_or_else(|| AgenticHarnessError::ModelNotFound {
                model: "(default)".to_string(),
                available: self.model_names(),
            })?;
        let key = ModelConfig::parse(&model)?.as_key();
        self.models
            .get(&key)
            .cloned()
            .ok_or_else(|| AgenticHarnessError::ModelNotFound {
                model: key,
                available: self.model_names(),
            })
    }

    fn model_names(&self) -> Vec<String> {
        self.models.keys().cloned().collect()
    }

    fn resolve_tool_defs(&self, per_call: &[ToolDef]) -> Result<Vec<ToolDef>, AgenticHarnessError> {
        validate_tool_defs(&self.tools, per_call)?;
        Ok(self.tools.iter().chain(per_call.iter()).cloned().collect())
    }

    #[cfg(feature = "native")]
    pub fn shell(&self, command: &str) -> Result<ShellOutput, AgenticHarnessError> {
        self.shell_with_options(command, ShellOptions::default())
    }

    #[cfg(feature = "native")]
    pub fn shell_with_options(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        let cwd = options
            .cwd
            .as_ref()
            .map(|cwd| self.resolve_workspace_path(cwd))
            .unwrap_or_else(|| self.workspace.clone());
        let child = Command::new("/bin/sh")
            .arg("-lc")
            .arg(command)
            .current_dir(cwd)
            .env_clear()
            .envs(default_shell_env())
            .envs(options.env)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let output = wait_with_timeout(child, options.timeout)?;
        Ok(shell_output(output))
    }

    #[cfg(feature = "native")]
    pub fn read(&self, path: &str) -> Result<String, AgenticHarnessError> {
        Ok(fs::read_to_string(self.resolve_workspace_path(path))?)
    }

    #[cfg(feature = "native")]
    pub fn read_with_options(
        &self,
        path: &str,
        options: ReadOptions,
    ) -> Result<ReadOutput, AgenticHarnessError> {
        bounded_read(
            &fs::read_to_string(self.resolve_workspace_path(path))?,
            options,
        )
    }

    #[cfg(feature = "native")]
    pub fn write(&self, path: &str, content: impl AsRef<[u8]>) -> Result<(), AgenticHarnessError> {
        let path = self.resolve_workspace_path(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, content)?;
        Ok(())
    }

    #[cfg(feature = "native")]
    pub fn edit(&self, path: &str, old: &str, new: &str) -> Result<(), AgenticHarnessError> {
        let absolute = self.resolve_workspace_path(path);
        let content = fs::read_to_string(&absolute)?;
        let occurrences = content.matches(old).count();
        if occurrences == 0 {
            return Err(AgenticHarnessError::Handler {
                message: format!("Could not find exact text in {path}."),
            });
        }
        if occurrences > 1 {
            return Err(AgenticHarnessError::Handler {
                message: format!(
                    "Found {occurrences} occurrences in {path}; provide a unique old text."
                ),
            });
        }
        fs::write(absolute, content.replacen(old, new, 1))?;
        Ok(())
    }

    #[cfg(feature = "native")]
    pub fn grep(&self, pattern: &str) -> Result<Vec<GrepMatch>, AgenticHarnessError> {
        let regex =
            regex::Regex::new(pattern).map_err(|err| AgenticHarnessError::InvalidRegex {
                message: err.to_string(),
            })?;
        let mut matches = Vec::new();
        for path in collect_files(&self.workspace)? {
            let Ok(content) = fs::read_to_string(&path) else {
                continue;
            };
            let rel = relative_path(&self.workspace, &path);
            for (index, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    matches.push(GrepMatch {
                        path: rel.clone(),
                        line: index + 1,
                        text: line.to_string(),
                    });
                }
            }
        }
        matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        Ok(matches)
    }

    #[cfg(feature = "native")]
    pub fn glob(&self, pattern: &str) -> Result<Vec<String>, AgenticHarnessError> {
        let mut paths = Vec::new();
        for path in collect_files(&self.workspace)? {
            let rel = relative_path(&self.workspace, &path);
            if glob_match(pattern, &rel) {
                paths.push(rel);
            }
        }
        paths.sort();
        Ok(paths)
    }

    #[cfg(feature = "native")]
    pub fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        file_stat(&self.resolve_workspace_path(path))
    }

    #[cfg(feature = "native")]
    pub fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.resolve_workspace_path(path))? {
            let entry = entry?;
            entries.push(entry.file_name().to_string_lossy().into_owned());
        }
        entries.sort();
        Ok(entries)
    }

    #[cfg(feature = "native")]
    pub fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        Ok(self.resolve_workspace_path(path).exists())
    }

    #[cfg(feature = "native")]
    pub fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        fs::create_dir_all(self.resolve_workspace_path(path))?;
        Ok(())
    }

    #[cfg(feature = "native")]
    pub fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        let path = self.resolve_workspace_path(path);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            if recursive {
                fs::remove_dir_all(path)?;
            } else {
                fs::remove_dir(path)?;
            }
        } else {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    #[cfg(feature = "native")]
    pub fn session(&self) -> Session {
        self.session_with_id("default")
    }

    #[cfg(feature = "native")]
    pub fn try_session(&self) -> Result<Session, AgenticHarnessError> {
        self.try_session_with_id("default")
    }

    #[cfg(feature = "native")]
    pub fn session_with_id(&self, session_id: &str) -> Session {
        self.session_with_shared_env(session_id, Arc::new(self.clone()))
    }

    #[cfg(feature = "native")]
    pub fn try_session_with_id(&self, session_id: &str) -> Result<Session, AgenticHarnessError> {
        self.try_session_with_shared_env(session_id, Arc::new(self.clone()))
    }

    pub fn session_with_env<E>(&self, env: E) -> Session
    where
        E: SessionEnv + 'static,
    {
        self.session_with_id_and_env("default", env)
    }

    pub fn try_session_with_env<E>(&self, env: E) -> Result<Session, AgenticHarnessError>
    where
        E: SessionEnv + 'static,
    {
        self.try_session_with_id_and_env("default", env)
    }

    pub fn session_with_id_and_env<E>(&self, session_id: &str, env: E) -> Session
    where
        E: SessionEnv + 'static,
    {
        self.session_with_shared_env(session_id, Arc::new(env))
    }

    pub fn try_session_with_id_and_env<E>(
        &self,
        session_id: &str,
        env: E,
    ) -> Result<Session, AgenticHarnessError>
    where
        E: SessionEnv + 'static,
    {
        self.try_session_with_shared_env(session_id, Arc::new(env))
    }

    fn session_with_shared_env(&self, session_id: &str, env: SharedSessionEnv) -> Session {
        let key = self.session_key(session_id);
        let data = self.load_session_data(&key).unwrap_or_default();
        self.build_session(key, data, env)
    }

    fn try_session_with_shared_env(
        &self,
        session_id: &str,
        env: SharedSessionEnv,
    ) -> Result<Session, AgenticHarnessError> {
        let key = self.session_key(session_id);
        let data = self.load_session_data(&key)?;
        Ok(self.build_session(key, data, env))
    }

    fn load_session_data(&self, key: &str) -> Result<SessionData, AgenticHarnessError> {
        let cached_data = self
            .sessions
            .lock()
            .map_err(|_| AgenticHarnessError::Handler {
                message: "session store lock poisoned".to_string(),
            })?
            .get(key)
            .cloned();
        if let Some(data) = cached_data {
            return Ok(data);
        }
        if let Some(store) = &self.persisted_sessions {
            return Ok(store.load(key)?.unwrap_or_default());
        }
        Ok(SessionData::default())
    }

    fn build_session(&self, key: String, data: SessionData, env: SharedSessionEnv) -> Session {
        let history = data.context_messages();
        Session {
            context: self.clone(),
            env,
            key,
            data,
            history,
        }
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), AgenticHarnessError> {
        let key = self.session_key(session_id);
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| AgenticHarnessError::Handler {
                message: "session store lock poisoned".to_string(),
            })?;
        sessions.remove(&key);
        if let Some(store) = &self.persisted_sessions {
            store.delete(&key)?;
        }
        Ok(())
    }

    fn save_session_data(&self, key: &str, data: &SessionData) -> Result<(), AgenticHarnessError> {
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| AgenticHarnessError::Handler {
                message: "session store lock poisoned".to_string(),
            })?;
        sessions.insert(key.to_string(), data.clone());
        if let Some(store) = &self.persisted_sessions {
            store.save(key, data)?;
        }
        Ok(())
    }

    fn session_key(&self, session_id: &str) -> String {
        format!("{}:{}", self.id, session_id)
    }

    #[cfg(feature = "native")]
    fn resolve_workspace_path(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.workspace.join(path)
        }
    }
}

#[cfg(feature = "native")]
impl SessionEnv for AgentContext {
    fn exec(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        self.shell_with_options(command, options)
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> {
        self.read(path)
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> {
        self.write(path, content)
    }

    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        self.stat(path)
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        self.readdir(path)
    }

    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        self.exists(path)
    }

    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        self.mkdir(path)
    }

    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        self.rm(path, recursive)
    }

    fn cwd(&self) -> &Path {
        self.workspace()
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        self.resolve_workspace_path(path)
    }
}

/// Registered Rust agent handler and metadata.
#[derive(Clone)]
pub struct AgentDefinition {
    name: String,
    triggers: Triggers,
    handler: Arc<Handler>,
}

impl AgentDefinition {
    pub fn webhook<F>(name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(AgentContext) -> AgentResult + Send + Sync + 'static,
    {
        Self::new(name, Triggers { webhook: true }, handler)
    }

    pub fn cli_only<F>(name: impl Into<String>, handler: F) -> Self
    where
        F: Fn(AgentContext) -> AgentResult + Send + Sync + 'static,
    {
        Self::new(name, Triggers { webhook: false }, handler)
    }

    pub fn new<F>(name: impl Into<String>, triggers: Triggers, handler: F) -> Self
    where
        F: Fn(AgentContext) -> AgentResult + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            triggers,
            handler: Arc::new(handler),
        }
    }
}

/// Native Agentic Harness app registry.
#[derive(Clone)]
pub struct AgentApp {
    workspace: PathBuf,
    agents: BTreeMap<String, AgentDefinition>,
    roles: BTreeMap<String, Role>,
    skills: BTreeMap<String, Skill>,
    commands: BTreeMap<String, CommandDef>,
    tools: Vec<ToolDef>,
    models: BTreeMap<String, SharedModelClient>,
    default_model: Option<String>,
    providers: ProvidersConfig,
    sessions: SharedSessionStore,
    persisted_sessions: Option<SharedPersistedSessionStore>,
}

impl Default for AgentApp {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentApp {
    pub fn new() -> Self {
        Self {
            workspace: PathBuf::from("."),
            agents: BTreeMap::new(),
            roles: BTreeMap::new(),
            skills: BTreeMap::new(),
            commands: BTreeMap::new(),
            tools: Vec::new(),
            models: BTreeMap::new(),
            default_model: None,
            providers: ProvidersConfig::default(),
            sessions: Arc::new(Mutex::new(BTreeMap::new())),
            persisted_sessions: None,
        }
    }

    pub fn with_workspace(mut self, workspace: impl AsRef<Path>) -> Self {
        self.workspace = workspace.as_ref().to_path_buf();
        self
    }

    pub fn agent(mut self, agent: AgentDefinition) -> Self {
        self.agents.insert(agent.name.clone(), agent);
        self
    }

    pub fn role(mut self, role: Role) -> Self {
        self.roles.insert(role.name.clone(), role);
        self
    }

    pub fn skill(mut self, skill: Skill) -> Self {
        self.skills.insert(skill.name.clone(), skill);
        self
    }

    pub fn command(mut self, command: CommandDef) -> Self {
        self.commands.insert(command.name.clone(), command);
        self
    }

    pub fn tool(mut self, tool: ToolDef) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn model<M: ModelClient + 'static>(mut self, model: impl AsRef<str>, client: M) -> Self {
        let config = ModelConfig::parse(model.as_ref()).unwrap_or_else(|err| {
            panic!("{err}");
        });
        self.models.insert(config.as_key(), Arc::new(client));
        self
    }

    pub fn default_model(mut self, model: impl AsRef<str>) -> Self {
        let config = ModelConfig::parse(model.as_ref()).unwrap_or_else(|err| {
            panic!("{err}");
        });
        self.default_model = Some(config.as_key());
        self
    }

    pub fn provider_settings(
        mut self,
        provider: impl Into<String>,
        settings: ProviderSettings,
    ) -> Self {
        self.providers.insert(provider, settings);
        self
    }

    pub fn providers(mut self, providers: ProvidersConfig) -> Self {
        self.providers = self.providers.merged(&providers);
        self
    }

    pub fn runtime_config(
        mut self,
        config: AgentRuntimeConfig,
    ) -> Result<Self, AgenticHarnessError> {
        self.providers = self.providers.merged(&config.providers);
        if let Some(default_model) = config.default_model {
            let parsed = ModelConfig::parse(&default_model)?;
            self.default_model = Some(parsed.as_key());
        }
        #[cfg(feature = "native")]
        for model in config.openai_compatible_models {
            let parsed = ModelConfig::parse(&model)?;
            let settings = self
                .providers
                .get(&parsed.provider)
                .cloned()
                .unwrap_or_default();
            self.models.insert(
                parsed.as_key(),
                Arc::new(OpenAiCompatibleModel::from_model_config(parsed, settings)),
            );
        }
        #[cfg(not(feature = "native"))]
        if let Some(model) = config.openai_compatible_models.into_iter().next() {
            ModelConfig::parse(&model)?;
            return Err(AgenticHarnessError::Provider {
                message: format!("OpenAI-compatible model {model:?} requires the native feature"),
            });
        }
        Ok(self)
    }

    #[cfg(feature = "native")]
    pub fn load_runtime_config(self, path: impl AsRef<Path>) -> Result<Self, AgenticHarnessError> {
        self.runtime_config(AgentRuntimeConfig::from_path(path)?)
    }

    pub fn default_model_name(&self) -> Option<&str> {
        self.default_model.as_deref()
    }

    pub fn model_names(&self) -> Vec<String> {
        self.models.keys().cloned().collect()
    }

    pub fn session_store<S: SessionStore + 'static>(mut self, store: S) -> Self {
        self.persisted_sessions = Some(Arc::new(store));
        self
    }

    #[cfg(feature = "native")]
    pub fn file_session_store(mut self, root: impl Into<PathBuf>) -> Self {
        self.persisted_sessions = Some(Arc::new(FileSessionStore::new(root)));
        self
    }

    #[cfg(feature = "native")]
    pub fn load_workspace_context(mut self) -> Result<Self, AgenticHarnessError> {
        for role in discover_roles(&self.workspace)? {
            self.roles.insert(role.name.clone(), role);
        }
        for skill in discover_skills(&self.workspace)? {
            self.skills.insert(skill.name.clone(), skill);
        }
        if let Some(config) = AgentRuntimeConfig::from_workspace(&self.workspace)? {
            self = self.runtime_config(config)?;
        }
        Ok(self)
    }

    pub fn manifest(&self) -> Manifest {
        Manifest {
            agents: self
                .agents
                .values()
                .map(|agent| ManifestAgent {
                    name: agent.name.clone(),
                    triggers: agent.triggers,
                })
                .collect(),
        }
    }

    pub fn cloudflare_worker_manifest(
        &self,
    ) -> Result<CloudflareWorkerManifest, AgenticHarnessError> {
        let mut durable_objects = Vec::new();
        let mut webhook_agent_names = Vec::new();
        for agent in self.agents.values().filter(|agent| agent.triggers.webhook) {
            validate_cloudflare_agent_name(&agent.name)?;
            let class_name = cloudflare_agent_class_name(&agent.name);
            durable_objects.push(CloudflareDurableObjectBinding {
                agent_name: agent.name.clone(),
                binding_name: class_name.clone(),
                class_name,
            });
            webhook_agent_names.push(agent.name.clone());
        }
        Ok(CloudflareWorkerManifest {
            durable_objects,
            webhook_agent_names,
        })
    }

    pub fn invoke(
        &self,
        name: &str,
        id: &str,
        payload: Value,
    ) -> Result<Value, AgenticHarnessError> {
        self.invoke_with_events(name, id, payload)
            .map(|(result, _events)| result)
    }

    pub fn invoke_with_event_sink<F>(
        &self,
        name: &str,
        id: &str,
        payload: Value,
        sink: F,
    ) -> Result<Value, AgenticHarnessError>
    where
        F: Fn(RuntimeEvent) + Send + Sync + 'static,
    {
        if !self.agents.contains_key(name) {
            return Err(AgenticHarnessError::AgentNotFound {
                name: name.to_string(),
                available: self.agent_names(),
            });
        }
        let sink: SharedRuntimeEvents = Arc::new(sink);
        sink(RuntimeEvent::new(
            "agent_start",
            json!({ "agent": name, "id": id }),
        ));
        let result = self.invoke_with_event_sink_inner(name, id, payload, sink.clone());
        match result {
            Ok(result) => {
                sink(RuntimeEvent::new("result", result.clone()));
                sink(RuntimeEvent::new("idle", json!({})));
                Ok(result)
            }
            Err(err) => {
                sink(RuntimeEvent::new("error", err.envelope()));
                sink(RuntimeEvent::new("idle", json!({})));
                Err(err)
            }
        }
    }

    pub fn handle_worker_app_request(&self, request: WorkerAppRequest) -> WorkerAppResponse {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let seen = captured.clone();
        let result = self.invoke_with_event_sink(
            &request.agent_name,
            &request.id,
            request.payload,
            move |event| {
                if let Ok(mut events) = seen.lock() {
                    events.push(event);
                }
            },
        );
        let events = captured
            .lock()
            .map(|events| events.clone())
            .unwrap_or_default();

        match result {
            Ok(result) => WorkerAppResponse {
                status: 200,
                result: Some(result),
                error: None,
                events,
            },
            Err(err) => WorkerAppResponse {
                status: err.status(),
                result: None,
                error: Some(err.envelope()),
                events,
            },
        }
    }

    fn invoke_with_events(
        &self,
        name: &str,
        id: &str,
        payload: Value,
    ) -> Result<(Value, Vec<RuntimeEvent>), AgenticHarnessError> {
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink_events = captured.clone();
        let sink: SharedRuntimeEvents = Arc::new(move |event| {
            if let Ok(mut events) = sink_events.lock() {
                events.push(event);
            }
        });
        let result = self.invoke_with_event_sink_inner(name, id, payload, sink)?;
        let events = captured
            .lock()
            .map_err(|_| AgenticHarnessError::Handler {
                message: "event store lock poisoned".to_string(),
            })?
            .clone();
        Ok((result, events))
    }

    fn invoke_with_event_sink_inner(
        &self,
        name: &str,
        id: &str,
        payload: Value,
        events: SharedRuntimeEvents,
    ) -> Result<Value, AgenticHarnessError> {
        let agent = self
            .agents
            .get(name)
            .ok_or_else(|| AgenticHarnessError::AgentNotFound {
                name: name.to_string(),
                available: self.agent_names(),
            })?;
        let ctx = AgentContext {
            id: id.to_string(),
            payload,
            workspace: self.workspace.clone(),
            roles: Arc::new(self.roles.clone()),
            skills: Arc::new(self.skills.clone()),
            commands: Arc::new(self.commands.clone()),
            tools: Arc::new(self.tools.clone()),
            models: Arc::new(self.models.clone()),
            default_model: self.default_model.clone(),
            providers: self.providers.clone(),
            sessions: self.sessions.clone(),
            persisted_sessions: self.persisted_sessions.clone(),
            events: events.clone(),
        };
        (agent.handler)(ctx)
    }

    pub fn handle_http(&self, method: &str, path: &str, body: &[u8]) -> HttpResponse {
        self.handle_request(HttpRequest::new(method, path, body))
    }

    pub fn handle_request(&self, request: HttpRequest) -> HttpResponse {
        match self.try_handle_request(&request) {
            Ok(response) => response,
            Err(err) => HttpResponse::json(err.status(), err.envelope()),
        }
    }

    fn try_handle_request(
        &self,
        request: &HttpRequest,
    ) -> Result<HttpResponse, AgenticHarnessError> {
        let method = request.method.as_str();
        let path = request.path.as_str();
        if method == "GET" && path == "/health" {
            return Ok(HttpResponse::json(200, json!({ "status": "ok" })));
        }
        if method == "GET" && path == "/agents" {
            return Ok(HttpResponse::json(
                200,
                serde_json::to_value(self.manifest())?,
            ));
        }

        let parts: Vec<&str> = path.trim_matches('/').split('/').collect();
        if (parts.len() == 3 || (parts.len() == 4 && parts[3] == "events")) && parts[0] == "agents"
        {
            if method != "POST" {
                return Err(AgenticHarnessError::MethodNotAllowed {
                    method: method.to_string(),
                });
            }
            let name = parts[1];
            let id = parts[2];
            let agent =
                self.agents
                    .get(name)
                    .ok_or_else(|| AgenticHarnessError::AgentNotFound {
                        name: name.to_string(),
                        available: self.agent_names(),
                    })?;
            if !agent.triggers.webhook {
                return Err(AgenticHarnessError::RouteNotFound {
                    method: method.to_string(),
                    path: path.to_string(),
                });
            }
            let payload = parse_json_body(&request.body)?;
            let wants_sse = parts.len() == 4 || request_accepts_sse(request);
            if wants_sse {
                let (result, runtime_events) = self.invoke_with_events(name, id, payload)?;
                let mut events = vec![(
                    "agent_start".to_string(),
                    json!({ "agent": name, "id": id }),
                )];
                events.extend(
                    runtime_events
                        .into_iter()
                        .map(|event| (event.event, event.data)),
                );
                events.push(("result".to_string(), result));
                events.push(("idle".to_string(), json!({})));
                return HttpResponse::sse(&events);
            }
            let result = self.invoke(name, id, payload)?;
            return Ok(HttpResponse::json(200, json!({ "result": result })));
        }

        Err(AgenticHarnessError::RouteNotFound {
            method: method.to_string(),
            path: path.to_string(),
        })
    }

    #[cfg(feature = "native")]
    pub fn serve(self, addr: impl AsRef<str>) -> Result<(), AgenticHarnessError> {
        let listener = TcpListener::bind(addr.as_ref())?;
        for stream in listener.incoming() {
            let stream = stream?;
            let app = self.clone();
            std::thread::spawn(move || {
                let _ = handle_stream(app, stream);
            });
        }
        Ok(())
    }

    fn agent_names(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }
}

fn parse_json_body(body: &[u8]) -> Result<Value, AgenticHarnessError> {
    if body.is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_slice(body).map_err(|err| AgenticHarnessError::InvalidJson {
        message: err.to_string(),
    })
}

fn request_accepts_sse(request: &HttpRequest) -> bool {
    request
        .header_value("accept")
        .map(|accept| {
            accept
                .split(',')
                .any(|value| value.trim().eq_ignore_ascii_case("text/event-stream"))
        })
        .unwrap_or(false)
}

fn validate_cloudflare_agent_name(name: &str) -> Result<(), AgenticHarnessError> {
    let valid = !name.is_empty()
        && name.split('-').all(|part| {
            let mut chars = part.chars();
            matches!(chars.next(), Some(first) if first.is_ascii_lowercase())
                && chars.all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit())
        });
    if valid {
        return Ok(());
    }
    Err(AgenticHarnessError::InvalidDeployment {
        target: "cloudflare".to_string(),
        message: format!("agent name {name:?} must be lower-kebab-case for Durable Object routing"),
    })
}

fn cloudflare_agent_class_name(name: &str) -> String {
    name.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            let Some(first) = chars.next() else {
                return String::new();
            };
            let mut class_part = first.to_ascii_uppercase().to_string();
            class_part.extend(chars);
            class_part
        })
        .collect()
}

#[cfg(feature = "native")]
fn collect_files(root: &Path) -> Result<Vec<PathBuf>, AgenticHarnessError> {
    let mut files = Vec::new();
    collect_files_inner(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn grep_env(env: &dyn SessionEnv, pattern: &str) -> Result<Vec<GrepMatch>, AgenticHarnessError> {
    let regex = regex::Regex::new(pattern).map_err(|err| AgenticHarnessError::InvalidRegex {
        message: err.to_string(),
    })?;
    let mut matches = Vec::new();
    for path in collect_env_files(env)? {
        let Ok(content) = env.read_file(&path) else {
            continue;
        };
        for (index, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                matches.push(GrepMatch {
                    path: path.clone(),
                    line: index + 1,
                    text: line.to_string(),
                });
            }
        }
    }
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Ok(matches)
}

fn glob_env(env: &dyn SessionEnv, pattern: &str) -> Result<Vec<String>, AgenticHarnessError> {
    let mut paths = collect_env_files(env)?
        .into_iter()
        .filter(|path| glob_match(pattern, path))
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

fn collect_env_files(env: &dyn SessionEnv) -> Result<Vec<String>, AgenticHarnessError> {
    let mut files = Vec::new();
    collect_env_files_inner(env, ".", &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_env_files_inner(
    env: &dyn SessionEnv,
    path: &str,
    files: &mut Vec<String>,
) -> Result<(), AgenticHarnessError> {
    let entries = match env.readdir(path) {
        Ok(entries) => entries,
        Err(err) if path == "." => {
            let _ = err;
            Vec::new()
        }
        Err(err) => return Err(err),
    };
    for entry in entries {
        let child = join_env_path(path, &entry);
        let stat = env.stat(&child)?;
        if stat.is_directory {
            collect_env_files_inner(env, &child, files)?;
        } else if stat.is_file {
            files.push(child);
        }
    }
    Ok(())
}

fn join_env_path(parent: &str, child: &str) -> String {
    if parent == "." || parent.is_empty() {
        child.to_string()
    } else {
        format!("{}/{}", parent.trim_end_matches('/'), child)
    }
}

#[cfg(feature = "native")]
fn collect_files_inner(path: &Path, files: &mut Vec<PathBuf>) -> Result<(), AgenticHarnessError> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files_inner(&path, files)?;
        } else if file_type.is_file() {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(feature = "native")]
fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn glob_match(pattern: &str, value: &str) -> bool {
    wildcard_match(pattern.as_bytes(), value.as_bytes())
}

fn wildcard_match(pattern: &[u8], value: &[u8]) -> bool {
    let mut states = vec![false; value.len() + 1];
    states[0] = true;

    for &pattern_byte in pattern {
        let mut next = vec![false; value.len() + 1];
        match pattern_byte {
            b'*' => {
                let mut active = false;
                for index in 0..=value.len() {
                    active |= states[index];
                    next[index] = active;
                }
            }
            b'?' => {
                next[1..(value.len() + 1)].copy_from_slice(&states[..value.len()]);
            }
            literal => {
                for index in 0..value.len() {
                    next[index + 1] = states[index] && value[index] == literal;
                }
            }
        }
        states = next;
    }

    states[value.len()]
}

fn extract_last_result_block(text: &str) -> Result<&str, AgenticHarnessError> {
    let mut rest = text;
    let mut last = None;

    while let Some(start) = rest.find("---RESULT_START---") {
        let after_start = &rest[start + "---RESULT_START---".len()..];
        let Some(end) = after_start.find("---RESULT_END---") else {
            break;
        };
        last = Some(after_start[..end].trim());
        rest = &after_start[end + "---RESULT_END---".len()..];
    }

    last.ok_or_else(|| AgenticHarnessError::ResultExtraction {
        message: "No ---RESULT_START--- / ---RESULT_END--- block found in the response."
            .to_string(),
    })
}

fn build_prompt_text(
    text: String,
    result_schema: Option<&Value>,
) -> Result<String, AgenticHarnessError> {
    let Some(schema) = result_schema else {
        return Ok(text);
    };
    Ok(format!("{text}\n{}", build_result_instructions(schema)?))
}

fn build_result_instructions(schema: &Value) -> Result<String, AgenticHarnessError> {
    let mut schema = schema.clone();
    if let Value::Object(object) = &mut schema {
        object.remove("$schema");
    }
    let schema = serde_json::to_string_pretty(&schema)?;
    Ok([
        "",
        "When complete, you MUST output your result between these exact delimiters conforming to this JSON schema:",
        "",
        "```json",
        schema.as_str(),
        "```",
        "",
        "Example: (Object)",
        "---RESULT_START---",
        "{\"key\":\"value\"}",
        "---RESULT_END---",
        "",
        "Example: (String)",
        "---RESULT_START---",
        "Hello, world!",
        "---RESULT_END---",
    ]
    .join("\n"))
}

fn validate_tool_defs(
    app_tools: &[ToolDef],
    per_call_tools: &[ToolDef],
) -> Result<(), AgenticHarnessError> {
    let mut seen = BTreeMap::new();
    for tool in app_tools.iter().chain(per_call_tools.iter()) {
        let name = tool.name();
        if BUILTIN_TOOL_NAMES.contains(&name) {
            return Err(AgenticHarnessError::ToolNameConflict {
                name: name.to_string(),
            });
        }
        if seen.insert(name.to_string(), ()).is_some() {
            return Err(AgenticHarnessError::DuplicateTool {
                name: name.to_string(),
            });
        }
    }
    Ok(())
}

fn execute_custom_model_tool_call(
    tools: &[ToolDef],
    tool_call: &ToolCall,
) -> Result<String, AgenticHarnessError> {
    let tool = tools
        .iter()
        .find(|tool| tool.name() == tool_call.name)
        .ok_or_else(|| AgenticHarnessError::ToolNotFound {
            name: tool_call.name.clone(),
            available: tools.iter().map(|tool| tool.name().to_string()).collect(),
        })?;
    tool.execute(tool_call.arguments.clone())
}

fn builtin_tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "read".to_string(),
            description:
                "Read a UTF-8 file. Supports optional byte offset and maxBytes for bounded reads."
                    .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "offset": { "type": "integer", "minimum": 0 },
                    "maxBytes": { "type": "integer", "minimum": 0 }
                },
                "required": ["path"]
            }),
        },
        ToolSpec {
            name: "write".to_string(),
            description: "Write UTF-8 content to a file, creating parent directories when needed."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
        },
        ToolSpec {
            name: "edit".to_string(),
            description: "Edit a file by replacing one exact text match.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "oldText": { "type": "string" },
                    "newText": { "type": "string" }
                },
                "required": ["path", "oldText", "newText"]
            }),
        },
        ToolSpec {
            name: "bash".to_string(),
            description: "Execute a shell command with optional cwd, env, and timeoutMs."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": { "type": "string" },
                    "cwd": { "type": "string" },
                    "env": {
                        "type": "object",
                        "additionalProperties": { "type": "string" }
                    },
                    "timeoutMs": { "type": "integer", "minimum": 0 }
                },
                "required": ["command"]
            }),
        },
        ToolSpec {
            name: "grep".to_string(),
            description: "Search files for a regex pattern and return matching lines.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" }
                },
                "required": ["pattern"]
            }),
        },
        ToolSpec {
            name: "glob".to_string(),
            description: "Find files by wildcard pattern using * and ?.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string" }
                },
                "required": ["pattern"]
            }),
        },
        ToolSpec {
            name: "task".to_string(),
            description: "Run a detached child task with its own session history.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string" },
                    "id": { "type": "string" }
                },
                "required": ["prompt"]
            }),
        },
    ]
}

fn required_string_arg(args: &Value, key: &str, tool: &str) -> Result<String, AgenticHarnessError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| AgenticHarnessError::Handler {
            message: format!("built-in tool {tool:?} requires string argument {key:?}"),
        })
}

fn optional_usize_arg(
    args: &Value,
    key: &str,
    tool: &str,
) -> Result<Option<usize>, AgenticHarnessError> {
    let Some(value) = args.get(key) else {
        return Ok(None);
    };
    let raw = value.as_u64().ok_or_else(|| AgenticHarnessError::Handler {
        message: format!("built-in tool {tool:?} argument {key:?} must be a non-negative integer"),
    })?;
    usize::try_from(raw)
        .map(Some)
        .map_err(|_| AgenticHarnessError::Handler {
            message: format!("built-in tool {tool:?} argument {key:?} is too large"),
        })
}

fn shell_options_from_tool_args(args: &Value) -> Result<ShellOptions, AgenticHarnessError> {
    let mut options = ShellOptions::new();
    if let Some(cwd) = args.get("cwd").and_then(Value::as_str) {
        options = options.cwd(cwd);
    }
    if let Some(timeout_ms) = optional_usize_arg(args, "timeoutMs", "bash")? {
        options = options.timeout(Duration::from_millis(timeout_ms as u64));
    }
    if let Some(env) = args.get("env") {
        let env = env
            .as_object()
            .ok_or_else(|| AgenticHarnessError::Handler {
                message: "built-in tool \"bash\" argument \"env\" must be an object".to_string(),
            })?;
        for (key, value) in env {
            let value = value.as_str().ok_or_else(|| AgenticHarnessError::Handler {
                message: format!("built-in tool \"bash\" env value for {key:?} must be a string"),
            })?;
            options = options.env(key, value);
        }
    }
    Ok(options)
}

fn assistant_tool_call_history(response: &PromptResponse) -> String {
    let calls = response
        .tool_calls
        .iter()
        .map(|call| {
            json!({
                "id": call.id,
                "name": call.name,
                "arguments": call.arguments
            })
        })
        .collect::<Vec<_>>();
    if response.text.trim().is_empty() {
        serde_json::to_string(&json!({ "toolCalls": calls })).unwrap_or_default()
    } else {
        serde_json::to_string(&json!({
            "text": response.text,
            "toolCalls": calls
        }))
        .unwrap_or_else(|_| response.text.clone())
    }
}

fn openai_tool_spec(tool: &ToolSpec) -> Value {
    json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters
        }
    })
}

fn parse_openai_tool_calls(message: &Value) -> Result<Vec<ToolCall>, AgenticHarnessError> {
    let Some(calls) = message.get("tool_calls").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };

    calls
        .iter()
        .enumerate()
        .map(|(index, call)| {
            let id = call
                .get("id")
                .and_then(Value::as_str)
                .map(ToString::to_string)
                .unwrap_or_else(|| format!("call-{index}"));
            let name = call
                .pointer("/function/name")
                .and_then(Value::as_str)
                .ok_or_else(|| AgenticHarnessError::Provider {
                    message: "provider tool call did not include function.name".to_string(),
                })?
                .to_string();
            let arguments = match call.pointer("/function/arguments") {
                Some(Value::String(raw)) if raw.trim().is_empty() => json!({}),
                Some(Value::String(raw)) => {
                    serde_json::from_str(raw).map_err(|err| AgenticHarnessError::Provider {
                        message: format!("provider tool call arguments were not valid JSON: {err}"),
                    })?
                }
                Some(value) => value.clone(),
                None => json!({}),
            };
            Ok(ToolCall {
                id,
                name,
                arguments,
            })
        })
        .collect()
}

#[cfg(feature = "native")]
fn default_shell_env() -> Vec<(String, String)> {
    const SAFE_ENV: &[&str] = &[
        "PATH", "HOME", "USER", "LOGNAME", "HOSTNAME", "SHELL", "LANG", "LC_ALL", "LC_CTYPE", "TZ",
        "TERM", "TMPDIR", "TMP", "TEMP",
    ];

    SAFE_ENV
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| ((*name).to_string(), value))
        })
        .collect()
}

#[cfg(feature = "native")]
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Option<Duration>,
) -> Result<Output, AgenticHarnessError> {
    let Some(timeout) = timeout else {
        return Ok(child.wait_with_output()?);
    };

    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let mut output = child.wait_with_output()?;
            if !output.stderr.is_empty() {
                output.stderr.push(b'\n');
            }
            output.stderr.extend_from_slice(
                format!(
                    "[agentic-harness] Command timed out after {}.",
                    format_duration(timeout)
                )
                .as_bytes(),
            );
            return Ok(output);
        }

        thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "native")]
fn format_duration(duration: Duration) -> String {
    if duration.as_millis() < 1_000 {
        format!("{}ms", duration.as_millis())
    } else if duration.subsec_millis() == 0 {
        format!("{}s", duration.as_secs())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(feature = "native")]
fn shell_output(output: Output) -> ShellOutput {
    ShellOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(1),
    }
}

fn bounded_read(content: &str, options: ReadOptions) -> Result<ReadOutput, AgenticHarnessError> {
    let total_bytes = content.len();
    if options.offset > total_bytes || !content.is_char_boundary(options.offset) {
        return Err(AgenticHarnessError::Handler {
            message: format!(
                "read offset {} is not a valid UTF-8 boundary for file with {total_bytes} bytes",
                options.offset
            ),
        });
    }

    let requested_end = options
        .max_bytes
        .map(|max_bytes| options.offset.saturating_add(max_bytes).min(total_bytes))
        .unwrap_or(total_bytes);
    let end = previous_char_boundary(content, requested_end);
    let selected = &content[options.offset..end];
    Ok(ReadOutput {
        content: selected.to_string(),
        start_byte: options.offset,
        bytes_read: selected.len(),
        total_bytes,
        truncated: end < total_bytes,
    })
}

fn previous_char_boundary(content: &str, mut index: usize) -> usize {
    index = index.min(content.len());
    while !content.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(feature = "native")]
fn file_stat(path: &Path) -> Result<FileStat, AgenticHarnessError> {
    let metadata = fs::symlink_metadata(path)?;
    let modified_unix_ms = metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_millis()).ok());

    Ok(FileStat {
        is_file: metadata.is_file(),
        is_directory: metadata.is_dir(),
        is_symbolic_link: metadata.file_type().is_symlink(),
        size: metadata.len(),
        modified_unix_ms,
    })
}

#[cfg(feature = "native")]
fn compose_system_prompt(
    workspace: &Path,
    skills: &BTreeMap<String, Skill>,
) -> Result<String, AgenticHarnessError> {
    let mut parts = Vec::new();

    for filename in ["AGENTS.md", "CLAUDE.md"] {
        let path = workspace.join(filename);
        if path.exists() {
            let content = fs::read_to_string(path)?;
            let content = content.trim();
            if !content.is_empty() {
                parts.push(content.to_string());
            }
        }
    }

    if !skills.is_empty() {
        parts.push("## Available Skills".to_string());
        for skill in skills.values() {
            let description = if skill.description.is_empty() {
                String::new()
            } else {
                format!(" - {}", skill.description)
            };
            parts.push(format!("- **{}**{}", skill.name, description));
        }
    }

    parts.push(format!("Working directory: {}", workspace.display()));
    let listing = directory_listing(workspace)?;
    if !listing.is_empty() {
        parts.push("Directory structure:".to_string());
        parts.extend(listing);
    }

    Ok(parts.join("\n\n"))
}

#[cfg(not(feature = "native"))]
fn compose_system_prompt(
    workspace: &Path,
    skills: &BTreeMap<String, Skill>,
) -> Result<String, AgenticHarnessError> {
    let mut parts = Vec::new();
    if !skills.is_empty() {
        parts.push("## Available Skills".to_string());
        for skill in skills.values() {
            let description = if skill.description.is_empty() {
                String::new()
            } else {
                format!(" - {}", skill.description)
            };
            parts.push(format!("- **{}**{}", skill.name, description));
        }
    }
    parts.push(format!("Working directory: {}", workspace.display()));
    Ok(parts.join("\n\n"))
}

#[cfg(feature = "native")]
fn directory_listing(workspace: &Path) -> Result<Vec<String>, AgenticHarnessError> {
    let mut entries = Vec::new();
    if !workspace.exists() {
        return Ok(entries);
    }
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        entries.push(entry.file_name().to_string_lossy().into_owned());
    }
    entries.sort();
    Ok(entries)
}

#[cfg(feature = "native")]
fn handle_stream(app: AgentApp, mut stream: TcpStream) -> Result<(), AgenticHarnessError> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body = vec![0; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let response = app.handle_http(&method, &path, &body);
    write_http_response(&mut stream, response)?;
    Ok(())
}

#[cfg(feature = "native")]
fn write_http_response(
    stream: &mut TcpStream,
    response: HttpResponse,
) -> Result<(), AgenticHarnessError> {
    let reason = match response.status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {} {}\r\ncontent-length: {}\r\n",
        response.status,
        reason,
        response.body.len()
    )?;
    for (name, value) in response.headers {
        write!(stream, "{name}: {value}\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.write_all(&response.body)?;
    Ok(())
}

#[cfg(feature = "native")]
fn discover_roles(workspace: &Path) -> Result<Vec<Role>, AgenticHarnessError> {
    let mut roles = Vec::new();
    for dir in [
        workspace.join(".agentic-harness/roles"),
        workspace.join("roles"),
    ] {
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if !matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("md" | "markdown")
            ) {
                continue;
            }
            let fallback_name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or("role")
                .to_string();
            let parsed = parse_frontmatter_file(&fs::read_to_string(&path)?, &fallback_name);
            roles.push(Role {
                name: fallback_name,
                description: parsed
                    .frontmatter
                    .get("description")
                    .cloned()
                    .unwrap_or_default(),
                instructions: parsed.body,
                model: parsed.frontmatter.get("model").cloned(),
            });
        }
    }
    Ok(roles)
}

#[cfg(feature = "native")]
fn discover_skills(workspace: &Path) -> Result<Vec<Skill>, AgenticHarnessError> {
    let mut skills = Vec::new();
    for root in [workspace.join(".agents/skills"), workspace.join("skills")] {
        if !root.exists() {
            continue;
        }
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir = entry.path();
            let skill_path = dir.join("SKILL.md");
            if !skill_path.exists() {
                continue;
            }
            let fallback_name = dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("skill")
                .to_string();
            let parsed = parse_frontmatter_file(&fs::read_to_string(skill_path)?, &fallback_name);
            let name = parsed
                .frontmatter
                .get("name")
                .cloned()
                .unwrap_or(fallback_name);
            skills.push(Skill {
                name,
                description: parsed
                    .frontmatter
                    .get("description")
                    .cloned()
                    .unwrap_or_default(),
                instructions: parsed.body,
            });
        }
    }
    Ok(skills)
}

#[cfg(feature = "native")]
struct ParsedMarkdown {
    frontmatter: HashMap<String, String>,
    body: String,
}

#[cfg(feature = "native")]
fn parse_frontmatter_file(content: &str, _fallback_name: &str) -> ParsedMarkdown {
    if let Some(rest) = content.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let raw = &rest[..end];
            let body = rest[end + "\n---".len()..]
                .strip_prefix('\n')
                .unwrap_or(&rest[end + "\n---".len()..])
                .to_string();
            return ParsedMarkdown {
                frontmatter: parse_frontmatter(raw),
                body,
            };
        }
    }
    ParsedMarkdown {
        frontmatter: HashMap::new(),
        body: content.to_string(),
    }
}

#[cfg(feature = "native")]
fn parse_frontmatter(raw: &str) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        values.insert(
            key.trim().to_string(),
            value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string(),
        );
    }
    values
}

/// Run a native Agentic Harness app using the embedded Rust-agent CLI.
///
/// Supported flags:
/// - `--agentic-harness-manifest`
/// - `--agentic-harness-cloudflare-manifest`
/// - `--agentic-harness-run <agent> --id <id> [--payload <json>]`
/// - `--agentic-harness-serve [--addr <host:port>]`
#[cfg(feature = "native")]
pub fn run_cli(app: AgentApp) -> Result<i32, AgenticHarnessError> {
    run_cli_with_args(app, std::env::args().skip(1))
}

#[cfg(feature = "native")]
pub fn run_cli_with_args<I, S>(app: AgentApp, args: I) -> Result<i32, AgenticHarnessError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    if has_any_flag(&args, &["--agentic-harness-manifest"]) {
        println!("{}", serde_json::to_string_pretty(&app.manifest())?);
        return Ok(0);
    }

    if has_any_flag(&args, &["--agentic-harness-cloudflare-manifest"]) {
        println!(
            "{}",
            serde_json::to_string_pretty(&app.cloudflare_worker_manifest()?)?
        );
        return Ok(0);
    }

    if has_any_flag(&args, &["--agentic-harness-serve"]) {
        let addr = flag_value(&args, "--addr").unwrap_or_else(|| "127.0.0.1:3583".to_string());
        app.serve(addr)?;
        return Ok(0);
    }

    if let Some(agent) = flag_value(&args, "--agentic-harness-run") {
        let id = flag_value(&args, "--id").unwrap_or_else(|| "default".to_string());
        let payload = match flag_value(&args, "--payload") {
            Some(raw) => {
                serde_json::from_str(&raw).map_err(|err| AgenticHarnessError::InvalidJson {
                    message: err.to_string(),
                })?
            }
            None => json!({}),
        };
        let result = app.invoke(&agent, &id, payload)?;
        println!("{}", serde_json::to_string_pretty(&result)?);
        return Ok(0);
    }

    eprintln!(
		"Usage:\n  --agentic-harness-manifest\n  --agentic-harness-cloudflare-manifest\n  --agentic-harness-run <agent> --id <id> [--payload <json>]\n  --agentic-harness-serve [--addr <host:port>]"
	);
    Ok(2)
}

#[cfg(feature = "native")]
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find_map(|window| (window[0] == flag).then(|| window[1].clone()))
}

#[cfg(feature = "native")]
fn has_any_flag(args: &[String], flags: &[&str]) -> bool {
    args.iter().any(|arg| flags.contains(&arg.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_handles_padding_cases() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(&[0xff, 0x00, 0x01]), "/wAB");
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_core_invokes_registered_handler() {
        let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
            Ok(json!({
                "id": ctx.id(),
                "payload": ctx.payload_value()
            }))
        }));

        let result = app
            .invoke("hello", "core-1", json!({ "ok": true }))
            .unwrap();

        assert_eq!(result["id"], "core-1");
        assert_eq!(result["payload"], json!({ "ok": true }));
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_core_accepts_custom_session_store() {
        #[derive(Clone, Default)]
        struct RecordingSessionStore {
            inner: Arc<Mutex<BTreeMap<String, SessionData>>>,
        }

        impl SessionStore for RecordingSessionStore {
            fn load(&self, key: &str) -> Result<Option<SessionData>, AgenticHarnessError> {
                Ok(self.inner.lock().unwrap().get(key).cloned())
            }

            fn save(&self, key: &str, data: &SessionData) -> Result<(), AgenticHarnessError> {
                self.inner
                    .lock()
                    .unwrap()
                    .insert(key.to_string(), data.clone());
                Ok(())
            }

            fn delete(&self, key: &str) -> Result<(), AgenticHarnessError> {
                self.inner.lock().unwrap().remove(key);
                Ok(())
            }
        }

        struct HistoryModel;

        impl ModelClient for HistoryModel {
            fn complete(
                &self,
                request: ModelRequest,
            ) -> Result<PromptResponse, AgenticHarnessError> {
                Ok(PromptResponse::text(format!(
                    "{} messages",
                    request.messages.len()
                )))
            }
        }

        fn app(store: RecordingSessionStore) -> AgentApp {
            AgentApp::new()
                .session_store(store)
                .agent(AgentDefinition::webhook("memory", |ctx| {
                    let mut session = ctx.session_with_env(MemorySessionEnv::new("."));
                    session.prompt(ctx.payload_value()["text"].as_str().unwrap(), &HistoryModel)?;
                    Ok(json!({ "historyLength": session.history().len() }))
                }))
        }

        let store = RecordingSessionStore::default();

        let first = app(store.clone())
            .invoke("memory", "same-id", json!({ "text": "first" }))
            .unwrap();
        let second = app(store.clone())
            .invoke("memory", "same-id", json!({ "text": "second" }))
            .unwrap();
        app(store.clone())
            .invoke("memory", "same-id", json!({ "text": "third" }))
            .unwrap();
        let saved = store
            .inner
            .lock()
            .unwrap()
            .values()
            .next()
            .cloned()
            .unwrap();

        assert_eq!(first["historyLength"], 2);
        assert_eq!(second["historyLength"], 4);
        assert_eq!(saved.entries.len(), 6);
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_try_session_with_env_surfaces_store_load_errors() {
        struct FailingSessionStore;

        impl SessionStore for FailingSessionStore {
            fn load(&self, _key: &str) -> Result<Option<SessionData>, AgenticHarnessError> {
                Err(AgenticHarnessError::Handler {
                    message: "session load failed".to_string(),
                })
            }

            fn save(&self, _key: &str, _data: &SessionData) -> Result<(), AgenticHarnessError> {
                Ok(())
            }

            fn delete(&self, _key: &str) -> Result<(), AgenticHarnessError> {
                Ok(())
            }
        }

        let app =
            AgentApp::new()
                .session_store(FailingSessionStore)
                .agent(AgentDefinition::webhook("memory", |ctx| {
                    let _session = ctx.try_session_with_env(MemorySessionEnv::new("."))?;
                    Ok(json!({ "unreachable": true }))
                }));

        let err = app.invoke("memory", "same-id", json!({})).unwrap_err();

        assert_eq!(err.error_type(), "handler_error");
        assert!(err.to_string().contains("session load failed"));
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_core_dispatches_sse_from_accept_header() {
        let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
            Ok(json!({ "id": ctx.id(), "payload": ctx.payload_value() }))
        }));

        let response = app.handle_request(
            HttpRequest::new("POST", "/agents/hello/worker-1", br#"{"x":1}"#)
                .header("Accept", "application/json, text/event-stream"),
        );
        let body = String::from_utf8(response.body).unwrap();

        assert_eq!(response.status, 200);
        assert!(response
            .headers
            .iter()
            .any(|(name, value)| name == "content-type" && value == "text/event-stream"));
        assert!(body.contains("event: agent_start\n"));
        assert!(body.contains("event: result\n"));
        assert!(body.contains("\"id\":\"worker-1\""));
        assert!(body.contains("event: idle\n"));
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_core_can_stream_runtime_events_to_a_sink() {
        struct TextModel;

        impl ModelClient for TextModel {
            fn complete(
                &self,
                _request: ModelRequest,
            ) -> Result<PromptResponse, AgenticHarnessError> {
                Ok(PromptResponse::text("streamed answer"))
            }
        }

        let app = AgentApp::new().agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session_with_env(MemorySessionEnv::new("."));
            let response = session.prompt("hello", &TextModel)?;
            Ok(json!({ "text": response.text }))
        }));
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();

        let result = app
            .invoke_with_event_sink("assistant", "live-1", json!({}), move |event| {
                seen.lock()
                    .unwrap()
                    .push((event.event().to_string(), event.data().clone()));
            })
            .unwrap();
        let events = events.lock().unwrap();
        let names = events
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();

        assert_eq!(result["text"], "streamed answer");
        assert_eq!(
            names,
            vec!["agent_start", "text_delta", "turn_end", "result", "idle"]
        );
        assert_eq!(events[1].1, json!({ "text": "streamed answer" }));
        assert_eq!(events[3].1["text"], "streamed answer");
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_openai_compatible_model_uses_custom_http_transport() {
        #[derive(Clone)]
        struct RecordingModelTransport {
            requests: Arc<Mutex<Vec<(String, BTreeMap<String, String>, Value)>>>,
        }

        impl HttpModelTransport for RecordingModelTransport {
            fn post_json(
                &self,
                url: &str,
                headers: &BTreeMap<String, String>,
                body: &Value,
            ) -> Result<HttpModelTransportResponse, AgenticHarnessError> {
                self.requests.lock().unwrap().push((
                    url.to_string(),
                    headers.clone(),
                    body.clone(),
                ));
                Ok(HttpModelTransportResponse {
                    status: 200,
                    body:
                        "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"worker ok\"}}]}"
                            .to_string(),
                })
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let model = OpenAiCompatibleModel::with_transport(
            "gpt-worker",
            ProviderSettings::new()
                .base_url("https://gateway.example/v1")
                .api_key("worker-key")
                .header("X-Worker", "yes"),
            RecordingModelTransport {
                requests: requests.clone(),
            },
        );

        let response = model
            .complete(ModelRequest {
                system: "worker system".to_string(),
                messages: vec![ModelMessage {
                    role: "user".to_string(),
                    content: "hello".to_string(),
                }],
                tools: vec![ToolSpec {
                    name: "lookup".to_string(),
                    description: "Look up a record".to_string(),
                    parameters: json!({
                        "type": "object",
                        "properties": {
                            "id": { "type": "string" }
                        }
                    }),
                }],
            })
            .unwrap();

        let requests = requests.lock().unwrap();
        let (url, headers, body) = &requests[0];

        assert_eq!(response.text, "worker ok");
        assert_eq!(url, "https://gateway.example/v1/chat/completions");
        assert_eq!(headers["Authorization"], "Bearer worker-key");
        assert_eq!(headers["X-Worker"], "yes");
        assert_eq!(body["model"], "gpt-worker");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "hello");
        assert_eq!(body["tools"][0]["function"]["name"], "lookup");
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_http_session_env_uses_custom_transport_for_remote_sandboxes() {
        #[derive(Clone)]
        struct RecordingSessionTransport {
            requests: Arc<Mutex<Vec<(String, BTreeMap<String, String>, Value)>>>,
        }

        impl HttpSessionTransport for RecordingSessionTransport {
            fn post_json(
                &self,
                url: &str,
                headers: &BTreeMap<String, String>,
                body: &Value,
            ) -> Result<HttpSessionTransportResponse, AgenticHarnessError> {
                self.requests.lock().unwrap().push((
                    url.to_string(),
                    headers.clone(),
                    body.clone(),
                ));
                Ok(HttpSessionTransportResponse {
                    status: 200,
                    body: serde_json::to_string(&ShellOutput {
                        stdout: "remote ok".to_string(),
                        stderr: String::new(),
                        exit_code: 0,
                    })?,
                })
            }
        }

        let requests = Arc::new(Mutex::new(Vec::new()));
        let env = HttpSessionEnv::with_transport(
            "https://sandbox.example/session",
            "/workspace",
            RecordingSessionTransport {
                requests: requests.clone(),
            },
        )
        .header("Authorization", "Bearer sandbox-token");

        let output = env
            .exec(
                "cargo test",
                ShellOptions::new()
                    .cwd("/workspace/project")
                    .env("RUST_LOG", "debug")
                    .timeout(Duration::from_secs(2)),
            )
            .unwrap();

        let requests = requests.lock().unwrap();
        let (url, headers, body) = &requests[0];

        assert_eq!(output.stdout, "remote ok");
        assert_eq!(url, "https://sandbox.example/session");
        assert_eq!(headers["Authorization"], "Bearer sandbox-token");
        assert_eq!(body["op"], "exec");
        assert_eq!(body["command"], "cargo test");
        assert_eq!(body["cwd"], "/workspace/project");
        assert_eq!(body["env"]["RUST_LOG"], "debug");
        assert_eq!(body["timeoutMs"], 2000);
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_core_handles_worker_app_json_abi_requests() {
        let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
            Ok(json!({ "id": ctx.id(), "payload": ctx.payload_value() }))
        }));

        let response = app.handle_worker_app_request(WorkerAppRequest::new(
            "hello",
            "wasm-1",
            json!({ "ok": true }),
        ));
        let event_names = response
            .events
            .iter()
            .map(|event| event.event())
            .collect::<Vec<_>>();

        assert_eq!(response.status, 200);
        assert_eq!(
            response.result,
            Some(json!({ "id": "wasm-1", "payload": { "ok": true } }))
        );
        assert_eq!(response.error, None);
        assert_eq!(event_names, vec!["agent_start", "result", "idle"]);

        let missing =
            app.handle_worker_app_request(WorkerAppRequest::new("missing", "wasm-2", json!({})));

        assert_eq!(missing.status, 404);
        assert_eq!(missing.result, None);
        assert_eq!(
            missing.error.as_ref().unwrap()["error"]["type"],
            "agent_not_found"
        );
    }

    #[cfg(not(feature = "native"))]
    mod worker_app_macro_exports {
        use super::*;

        fn app() -> AgentApp {
            AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
                Ok(json!({ "id": ctx.id(), "payload": ctx.payload_value() }))
            }))
        }

        crate::agentic_harness_worker_app!(app);
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_worker_app_macro_exports_wasm_json_abi() {
        let request = serde_json::to_vec(&WorkerAppRequest::new(
            "hello",
            "macro-1",
            json!({ "ok": true }),
        ))
        .unwrap();

        unsafe {
            let request_ptr = worker_app_macro_exports::agentic_harness_alloc(request.len());
            std::ptr::copy_nonoverlapping(request.as_ptr(), request_ptr, request.len());
            let result_ptr =
                worker_app_macro_exports::agentic_harness_invoke(request_ptr, request.len());
            let result_len = worker_app_macro_exports::agentic_harness_last_result_len();
            let result = std::slice::from_raw_parts(result_ptr, result_len);
            let response: WorkerAppResponse = serde_json::from_slice(result).unwrap();
            worker_app_macro_exports::agentic_harness_dealloc(request_ptr, request.len());

            assert_eq!(response.status, 200);
            assert_eq!(
                response.result,
                Some(json!({ "id": "macro-1", "payload": { "ok": true } }))
            );
            assert_eq!(response.error, None);
            assert_eq!(
                response
                    .events
                    .iter()
                    .map(RuntimeEvent::event)
                    .collect::<Vec<_>>(),
                vec!["agent_start", "result", "idle"]
            );
        }
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_runtime_event_encodes_sse_frames() {
        let event = RuntimeEvent::new("text_delta", json!({ "text": "hello" }));

        let frame = event.to_sse_frame().unwrap();

        assert_eq!(frame, "event: text_delta\ndata: {\"text\":\"hello\"}\n\n");
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_core_exposes_cloudflare_worker_bindings_for_webhook_agents() {
        let app = AgentApp::new()
            .agent(AgentDefinition::webhook("hello-world", |_| {
                Ok(json!({ "ok": true }))
            }))
            .agent(AgentDefinition::cli_only("triage", |_| {
                Ok(json!({ "ok": true }))
            }));

        let manifest = app.cloudflare_worker_manifest().unwrap();

        assert_eq!(
            manifest.durable_objects,
            vec![CloudflareDurableObjectBinding {
                agent_name: "hello-world".to_string(),
                binding_name: "HelloWorld".to_string(),
                class_name: "HelloWorld".to_string(),
            }]
        );
        assert_eq!(manifest.webhook_agent_names, vec!["hello-world"]);
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_cloudflare_manifest_generates_wrangler_config_fragment() {
        let manifest = CloudflareWorkerManifest {
            durable_objects: vec![CloudflareDurableObjectBinding {
                agent_name: "hello-world".to_string(),
                binding_name: "HelloWorld".to_string(),
                class_name: "HelloWorld".to_string(),
            }],
            webhook_agent_names: vec!["hello-world".to_string()],
        };

        let config = manifest.wrangler_config("demo-agent", "_entry.js");

        assert_eq!(
            config,
            json!({
                "$schema": "https://workers.cloudflare.com/schema/wrangler.json",
                "name": "demo-agent",
                "main": "_entry.js",
                "durable_objects": {
                    "bindings": [
                        {
                            "name": "HelloWorld",
                            "class_name": "HelloWorld"
                        }
                    ]
                },
                "migrations": [
                    {
                        "tag": "agentic-harness-HelloWorld",
                        "new_sqlite_classes": ["HelloWorld"]
                    }
                ]
            })
        );
    }

    #[cfg(not(feature = "native"))]
    #[test]
    fn no_default_cloudflare_manifest_generates_non_proxy_worker_entrypoint() {
        let manifest = CloudflareWorkerManifest {
            durable_objects: vec![CloudflareDurableObjectBinding {
                agent_name: "hello-world".to_string(),
                binding_name: "HelloWorld".to_string(),
                class_name: "HelloWorld".to_string(),
            }],
            webhook_agent_names: vec!["hello-world".to_string()],
        };

        let source = manifest.worker_entrypoint("./agentic_harness_worker.js");

        assert!(source.contains("import initWorker, { handleAgenticHarnessDurableObject } from './agentic_harness_worker.js';"));
        assert!(source.contains("const WEBHOOK_AGENT_NAMES = new Set([\"hello-world\"]);"));
        assert!(
            source.contains("const DURABLE_OBJECT_BINDINGS = {\"hello-world\":\"HelloWorld\"};")
        );
        assert!(source.contains("export class HelloWorld"));
        assert!(source.contains("static agentName = \"hello-world\";"));
        assert!(source.contains("stub.fetch(request);"));
        assert!(!source.contains("127.0.0.1"));
        assert!(!source.contains("localhost"));
    }

    #[cfg(feature = "native")]
    #[test]
    fn legacy_sse_endpoint_event_is_parsed_from_event_stream() {
        let endpoint =
            parse_sse_endpoint(": ready\n\nevent: endpoint\ndata: /message\n\n").unwrap();

        assert_eq!(endpoint, "/message");
    }

    #[cfg(feature = "native")]
    #[test]
    fn legacy_sse_relative_endpoint_resolves_against_server_url() {
        let endpoint =
            resolve_legacy_sse_endpoint("https://mcp.example/sse", "/message".to_string()).unwrap();

        assert_eq!(endpoint, "https://mcp.example/message");
    }
}
