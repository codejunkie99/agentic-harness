use agentic_harness::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct HelloPayload {
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TriagePayload {
    issue: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodingPayload {
    repo: Option<String>,
    prompt: Option<String>,
    #[serde(rename = "gitStatusBefore")]
    git_status_before: Option<String>,
    #[serde(rename = "gitDiffStat")]
    git_diff_stat: Option<String>,
    #[serde(rename = "gitChangedFiles")]
    git_changed_files: Option<Vec<String>>,
    #[serde(rename = "rootFiles")]
    root_files: Option<Vec<String>>,
    #[serde(rename = "projectFiles")]
    project_files: Option<Vec<String>>,
    #[serde(rename = "workspaceInstructions")]
    workspace_instructions: Option<Vec<WorkspaceInstructionPayload>>,
    checks: Option<Vec<String>>,
    #[serde(rename = "plannedSteps")]
    planned_steps: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceInstructionPayload {
    path: String,
    content: String,
}

struct NativeEchoModel;

impl ModelClient for NativeEchoModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let prompt = request
            .messages
            .last()
            .map(|message| message.content.as_str())
            .unwrap_or("");
        Ok(PromptResponse::text(format!(
            "Native model placeholder received: {prompt}"
        )))
    }
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new()
        .with_workspace(".")
        .load_workspace_context()?
        .model("local/echo", NativeEchoModel)
        .default_model("local/echo")
        .agent(AgentDefinition::webhook(
            "assistant",
            |ctx: AgentContext| {
                let mut session = ctx.session();
                let response = session.prompt_with_options(
                    "Explain what this Rust-native Agentic Harness example proves in one sentence.",
                    PromptOptions::new(),
                )?;
                Ok(json!({
                    "id": ctx.id(),
                    "text": response.text,
                    "historyLength": session.history().len()
                }))
            },
        ))
        .agent(AgentDefinition::webhook("hello", |ctx: AgentContext| {
            let payload: HelloPayload = ctx.payload()?;
            let name = payload.name.unwrap_or_else(|| "World".to_string());
            let role = ctx.require_role("greeter")?;
            Ok(json!({
                "id": ctx.id(),
                "message": format!("Hello, {name}!"),
                "role": role.description
            }))
        }))
        .agent(AgentDefinition::webhook("shell", |ctx: AgentContext| {
            let output = ctx.shell("cat AGENTS.md")?;
            let scoped = ctx.shell_with_options(
                "printf '%s' \"$AGENTIC_HARNESS_EXAMPLE\"",
                ShellOptions::new()
                    .env("AGENTIC_HARNESS_EXAMPLE", "scoped")
                    .timeout(Duration::from_secs(1)),
            )?;
            Ok(json!({
                "id": ctx.id(),
                "context": output.stdout.trim(),
                "exitCode": output.exit_code,
                "scoped": scoped.stdout
            }))
        }))
        .agent(AgentDefinition::webhook("env", |ctx: AgentContext| {
            Ok(json!({
                "id": ctx.id(),
                "value": std::env::var("AGENTIC_HARNESS_NATIVE_TEST").unwrap_or_default()
            }))
        }))
        .agent(AgentDefinition::webhook("code", |ctx: AgentContext| {
            let payload: CodingPayload = ctx.payload()?;
            let repo = payload.repo.unwrap_or_else(|| ".".to_string());
            let prompt = payload
                .prompt
                .unwrap_or_else(|| "Inspect the project".to_string());
            let checks = payload.checks.unwrap_or_default();
            let planned_steps = payload.planned_steps.unwrap_or_else(|| {
                vec![
                    "Read workspace instructions and current repository status.".to_string(),
                    "Apply the smallest focused changes needed for the prompt.".to_string(),
                    "Run the configured checks and summarize remaining risks.".to_string(),
                ]
            });
            let status = ctx.shell_with_options(
                "git status --short || true",
                ShellOptions::new()
                    .cwd(&repo)
                    .timeout(Duration::from_secs(5)),
            )?;
            Ok(json!({
                "id": ctx.id(),
                "repo": repo,
                "prompt": prompt,
                "status": status.stdout,
                "inspect": {
                    "gitStatusBefore": payload.git_status_before.unwrap_or_default(),
                    "gitDiffStat": payload.git_diff_stat.unwrap_or_default(),
                    "gitChangedFiles": payload.git_changed_files.unwrap_or_default(),
                    "rootFiles": payload.root_files.unwrap_or_default(),
                    "projectFiles": payload.project_files.unwrap_or_default(),
                    "workspaceInstructions": payload.workspace_instructions.unwrap_or_default()
                },
                "plan": planned_steps,
                "plannedSteps": planned_steps,
                "checks": checks,
                "summary": "Coding loop inspected the repository and prepared a local check plan."
            }))
        }))
        .agent(AgentDefinition::cli_only("triage", |ctx: AgentContext| {
            let payload: TriagePayload = ctx.payload()?;
            let issue = payload.issue.unwrap_or_else(|| "unspecified".to_string());
            Ok(json!({
                "id": ctx.id(),
                "issue": issue,
                "severity": "low",
                "summary": "Native Rust CLI-only agent executed without exposing an HTTP webhook."
            }))
        })))
}

fn main() {
    let code = match app().and_then(run_cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("[agentic-harness] {err}");
            1
        }
    };
    std::process::exit(code);
}
