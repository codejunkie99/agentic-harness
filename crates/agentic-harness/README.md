# agentic-harness

Native Rust SDK for Agentic Harness.

The SDK exposes the first-class Rust agent surface:

- `AgentApp` registers agents and owns workspace context.
- `AgentDefinition` declares webhook or CLI-only agents.
- `AgentContext` gives handlers typed payloads, request ids, roles, skills, shell execution, filesystem helpers, and sessions.
- HTTP dispatch supports JSON responses and an SSE event route at `/agents/<name>/<id>/events` with tool and text events.
- `HttpRequest` plus `AgentApp::handle_request` give Worker/native adapters a
  platform-neutral request entrypoint, including `Accept: text/event-stream`
  routing on `/agents/<name>/<id>`.
- `AgentApp::invoke_with_event_sink` sends runtime events to a live sink during
  handler execution, so non-native adapters can bridge events to platform
  streams without waiting for the handler to finish. `RuntimeEvent::to_sse_frame`
  exposes the reusable SSE frame encoder.
- `AgentApp::cloudflare_worker_manifest` derives webhook-agent Durable Object
  bindings/classes for Worker adapters and validates the lower-kebab-case names
  needed for Cloudflare routing. `CloudflareWorkerManifest::wrangler_config`
  emits the corresponding Wrangler JSON fragment, and `worker_entrypoint` emits
  the non-proxy Worker routing module.
- Workspace context follows Claude Code/Codex conventions: `AGENTS.md`, optional `CLAUDE.md`, available skills, cwd, and directory listing.
- Native file/search/edit helpers are available as `read`, bounded `read_with_options`, `write`, `edit`, `stat`, `readdir`, `exists`, `mkdir`, `rm`, `grep`, and `glob`.
- `shell_with_options` scopes command cwd, env, and timeout without exposing the full host environment by default.
- `CommandDef` registers explicit host capabilities that handlers call with `AgentContext::command`.
- `ToolDef` registers custom model tools with JSON parameter schemas, prompt-scoped metadata, and `ToolCall` execution loops.
- Models also receive built-in `read`, `write`, `edit`, `bash`, `grep`, `glob`, and `task` tools for the active session environment.
- `connect_mcp_server` and `mcp_tools_from_client` expose streamable HTTP or legacy SSE MCP tools as ordinary `ToolDef`s.
- `SessionEnv` is the Rust contract for local or remote sandbox connectors, and handlers can bind one with `AgentContext::session_with_id_and_env`.
- `MemorySessionEnv` provides an empty in-memory filesystem for isolated file/search helpers without host workspace access.
- `HttpSessionEnv` adapts remote sandbox services that expose shell and filesystem operations over JSON HTTP.
- `AgentApp::model`, `default_model`, and `PromptOptions` provide the `provider/model` runtime selection path.
- `AgentRuntimeConfig`, `AgentApp::load_runtime_config`, and workspace config discovery load `defaultModel`, `providers`, and `openaiCompatibleModels` from JSON.
- `OpenAiCompatibleModel` provides a built-in native Rust HTTP client for OpenAI-compatible providers and gateways, plus `with_transport` for non-native adapters that need to use platform HTTP.
- `AgentApp::session_store` accepts runtime-specific `SessionStore` persistence;
  `AgentApp::file_session_store` provides the native filesystem implementation.
- `AgentContext::try_session`, `try_session_with_id`, and
  `try_session_with_id_and_env` surface storage load errors for runtimes where
  persistence failures must abort the request.
- `Session` provides provider-agnostic `prompt`, `skill`, `task`, `shell`, structured result extraction, history helpers, manual and automatic compaction summaries, and branch summaries.
- `PromptOptions::result_schema` appends JSON-schema result guidance to model prompts; `prompt_json_with_options` then extracts the final result block into a typed Rust value.
- `ModelClient` is the provider boundary. Implement it for OpenAI, Anthropic, local models, or internal gateways without changing agent handlers.
- `run_cli` embeds `manifest`, `run`, and `serve` behavior into each Rust agent binary.

```rust
use agentic_harness::{CompactionSettings, PromptOptions};

let response = session.prompt_with_options(
    "Continue the investigation.",
    PromptOptions::new().compaction(
        CompactionSettings::new()
            .context_window_tokens(128_000)
            .reserve_tokens(16_384)
            .keep_recent_messages(12),
    ),
)?;
```

```rust
use agentic_harness::prelude::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct HelloPayload {
    name: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new()
        .with_workspace(".")
        .load_workspace_context()?
        .agent(AgentDefinition::webhook("hello", |ctx: AgentContext| {
            let payload: HelloPayload = ctx.payload()?;
            let name = payload.name.unwrap_or_else(|| "World".to_string());
            Ok(json!({ "id": ctx.id(), "message": format!("Hello, {name}!") }))
        })))
}

fn main() {
    std::process::exit(app().and_then(run_cli).unwrap_or(1));
}
```
