# Feature Status

Evidence-based status of the major capabilities in Agentic Harness. Each row
points at the Rust code, tests, or docs that cover the current behavior.

## Summary

Agentic Harness is a native Rust SDK and CLI for local, self-hosted, CI, and
remote-sandbox agent workflows. The primary product direction is coding agents
running against a local checkout or a remote Linux sandbox such as Vercel
Sandbox, Daytona, or E2B. Cloudflare is optional edge control/webhook
infrastructure, not the main coding-agent runtime. The target split is
documented in [`execution-targets.md`](execution-targets.md), with deploy guides
for Node hosts, Cloudflare Workers, GitHub Actions, and GitLab CI.

The current primary deployment path is a native Rust binary. `--target node`
builds a Node host package around that native binary for platforms that expect
`node server.mjs`. `--target cloudflare` is build-only: it writes non-proxy
Worker boundary artifacts and deploy-ready Wrangler metadata, but `run` and
`dev` fail explicitly instead of pretending a native binary can deploy to
Workers.

## Capability Matrix

Each capability below lists a status sentence and the code, tests, or docs that
support that status. Treat this file as a release-facing evidence map, not a
roadmap: future work belongs in [`immediate-goals.md`](immediate-goals.md).

### 1. Model/provider Runtime

- Status: Covered for native Rust plus non-native HTTP transport boundaries.
- Evidence: `ProviderSettings`, `ModelConfig`, `AgentRuntimeConfig`,
  `OpenAiCompatibleModel`, `HttpModelTransport`; tests `configured_models_*`,
  `provider_settings_*`, `openai_compatible_model_*`, `runtime_config_*`.

### 2. Tool Calling

- Status: Covered with custom tools, schemas, duplicate checks, model-requested
  calls, and built-ins.
- Evidence: `ToolDef`, `ToolSpec`, `ToolCall`, `execute_tool_call`,
  `builtin_tool_specs`; tests `custom_tools_*`,
  `session_executes_model_requested_tool_calls_before_returning_final_response`.

### 3. Sandbox/command Isolation

- Status: Covered with `SessionEnv`, host command registration, shell options,
  in-memory envs, `VirtualSessionEnv`, `HttpSessionEnv`, and provider-scoped
  `SandboxConnector` helpers for Vercel, Daytona, and E2B.
- Evidence: `SessionEnv`, `MemorySessionEnv`, `VirtualSessionEnv`,
  `HttpSessionEnv`, `SandboxConnector`, `HttpSessionTransport`, `CommandDef`,
  `ShellOptions`; docs `http-session-env.md`, `virtual-sandbox.md`,
  `connectors.md`; tests `virtual_session_env_*`, `sandbox_connector_*`,
  `http_session_env_*`, `shell_options_*`.

### 4. Child Delegation

- Status: Covered with detached child histories and model-visible task calls.
- Evidence: `Session::task`, `Session::task_with_id`, builtin `task`; tests
  `session_task_runs_in_detached_child_history`,
  `session_exposes_builtin_tools_to_models_and_executes_builtin_tool_calls`.

### 5. Structured Results

- Status: Covered with result block extraction and schema-guided typed JSON
  output.
- Evidence: `PromptResponse::result_json`, `PromptOptions::result_schema`,
  `Session::prompt_json_with_options`; tests
  `prompt_response_extracts_last_structured_json_result`,
  `prompt_options_can_add_schema_guidance_and_return_typed_json`.

### 6. Compaction/session History

- Status: Covered with persisted `SessionData`, pluggable `SessionStore`, native
  file-backed storage, manual compaction, branch summaries, and token-aware
  auto-compaction.
- Evidence: `SessionData`, `SessionEntry`, `SessionStore`, `FileSessionStore`,
  `CompactionSettings`; tests `file_session_store_*`,
  `prompt_options_auto_compact_over_budget_context_before_model_call`,
  `no_default_core_accepts_custom_session_store`.

### 7. MCP

- Status: Covered for streamable HTTP and legacy SSE MCP tool adaptation.
- Evidence: `McpServerOptions`, `McpTransport`, `connect_mcp_server`,
  `mcp_tools_from_client`; tests `mcp_client_tools_are_exposed_as_custom_tools`,
  `mcp_server_options_expose_legacy_sse_transport_selection`.

### 8. HTTP Streaming/events

- Status: Covered for native JSON, SSE endpoints, platform-neutral request
  handling, live event sinks, and reusable SSE frame encoding.
- Evidence: `AgentApp::handle_http`, `AgentApp::handle_request`,
  `AgentApp::invoke_with_event_sink`, `HttpRequest`, `RuntimeEvent`; tests
  `http_dispatch_*`, `runtime_events_can_stream_to_a_live_sink`,
  `runtime_event_encodes_sse_frames`.

### 9. Cloudflare/Node Build Targets

- Status: Covered for current scope. `native` is the real target, `node` builds
  a Node host package around the native binary, and `cloudflare` emits non-proxy
  Worker boundary files plus Wrangler metadata while `run` remains rejected.
- Evidence: `BuildTarget::parse`, `build_native`, `write_node_host_package`,
  `build_cloudflare`, `build_worker_wasm_crate`, Worker manifest types; docs
  `deploy-node.md`, `deploy-cloudflare.md`; tests
  `node_target_builds_node_host_package_for_native_binary`,
  `cloudflare_build_*`, `cloudflare_worker_manifest_*`.

### 10. Local Hosting and Dev Reload

- Status: Covered for native local hosting and watch/reload development.
- Evidence: `Commands::Host`, `Commands::Hosting`, `setup_hosting_command`,
  `hosting_status`, `run_dev_server`, `watch_snapshot`; tests
  `local_hosting_setup_status_and_dashboard_json_expose_urls`,
  `watch_snapshot_*`.

### 11. Connector Registry

- Status: Covered for native connector instructions, category roots, aliases,
  and agent-readable output when piped.
- Evidence: `native_connectors`, `native_category_roots`, provider connector
  markdown functions, `resolve_native_connector`,
  `should_print_agent_instructions`; tests
  `add_lists_and_prints_native_connector_instructions`,
  `add_category_roots_emit_agent_ready_connector_instructions`.

### 12. File/search Helpers

- Status: Covered with bounded reads, write/edit/stat/readdir/exists/mkdir/rm,
  regex grep, wildcard glob, and scoped shell options.
- Evidence: `ReadOptions`, `ReadOutput`, file helpers, `grep_env`, `glob_env`;
  tests `context_exposes_native_file_search_and_edit_tools`,
  `read_with_options_returns_bounded_content_and_truncation_metadata`,
  `grep_supports_regex_and_glob_supports_question_mark`.

### 13. Software Lifecycle SDK

- Status: Covered with first-class repo inspection, instruction loading,
  planning, patch application, check execution, summaries, commit handoff, and
  PR handoff.
- Evidence: `SoftwareWorkspace`, `RepoInspection`, `SoftwareCheck`,
  `SoftwarePlan`, `SoftwarePatch`, `SoftwareSummary`, `SoftwareCommit`,
  `SoftwarePullRequest`; test
  `software_workspace_sdk_exposes_repo_lifecycle_primitives`.

### 14. Starter Templates

- Status: Covered for native Rust starter projects including coding, review,
  test fixing, docs, refactor, release, repo analysis, support, triage, data,
  and hello-world shapes.
- Evidence: `Commands::New`, `ScaffoldTemplate`, `BUILT_IN_TEMPLATES`, scaffold
  functions; tests `new_scaffolds_a_native_rust_agent_project`,
  `new_scaffolds_source_style_agent_templates`.

### 15. TUI/dashboard and Onboarding

- Status: Covered with `start`, `wizard`, `tui`, `ui`, dashboard/status JSON,
  guide/onboard flows, contextual second-level panels, local-hosting status,
  sandbox status, template registry state, and doctor/check readiness.
- Evidence: `Commands::Start`, `Commands::Wizard`, `Commands::Dashboard`,
  `Commands::Doctor`, wizard render functions, dashboard/doctor formatters;
  tests `start_opens_the_guided_tui_front_door`, `wizard_*`, `dashboard_*`,
  `doctor_*`.

### 16. Template, LLM Setup, Sandbox Setup, and Coding Loop

- Status: Covered with template authoring/install/export/import/search, LLM
  handoff for Claude Code/Codex/Cursor/Wind Server, sandbox operations, remote
  check execution, one-command coding, repair pass, commit/PR gates, allow/deny
  path policy, dependency approval, command-risk gates, and durable run
  artifacts.
- Evidence: `Commands::Code`, `Commands::Template`, `Commands::Setup`,
  `Commands::Sandbox`, `code_command`, policy helpers, run artifact writers,
  template helpers; tests `template_*`, `setup_*`, `sandbox_*`,
  `code_command_*`.

### 17. HarnessScore Evaluation

- Status: Covered with native run scoring, legacy `agent_logs` scoring,
  Markdown/JSON reports, `code --score`, threshold gates, dashboard exposure,
  and latest score artifacts.
- Evidence: `crates/agentic-harness-score`, `Commands::Score`,
  `score_command`, `write_score_artifacts_for_run`,
  `latest_harness_score_json`; tests in `crates/agentic-harness-score/tests/`
  plus CLI tests `score_command_*`, `dashboard_exposes_latest_harness_score`,
  and `code_command_writes_mandatory_run_artifact_bundle`.

### 18. Release Packaging

- Status: Covered for pre-release distribution checks and local binary package
  staging.
- Evidence: `scripts/install.sh`, `Formula/agentic-harness.rb`, `CHANGELOG.md`,
  `docs/release-smoke-test.md`, `Commands::Smoke`, `Commands::Package`,
  `Commands::ReleaseCheck`; tests
  `release_packaging_artifacts_are_locally_smoke_testable`,
  `package_command_creates_binary_distribution_manifest_and_checksums`,
  `release_check_reports_packaging_readiness_as_text_and_json`,
  `smoke_command_reports_install_readiness_as_text_and_json`.

### 19. Migration and Deployment Examples

- Status: Covered with dedicated docs for Node hosts, Cloudflare Workers, GitHub
  Actions, GitLab CI, sandbox connectors, virtual sandboxing, and migration from
  prior TypeScript concepts to native Rust concepts.
- Evidence: `deploy-node.md`, `deploy-cloudflare.md`,
  `deploy-github-actions.md`, `deploy-gitlab-ci.md`, `connectors.md`,
  `virtual-sandbox.md`, migration guide; test
  `replacement_gap_docs_cover_deploy_connectors_virtual_sandbox_and_migration`.

## Deliberate Non-Goal

Cloudflare Workers are not the primary coding-agent execution target. The Rust
CLI generates Worker/Durable Object boundary artifacts, Wrangler config, session
plumbing, WASM ABI glue, and transport seams for Worker-safe model and sandbox
HTTP. Full Workers handler execution requires a real Worker-compatible adapter.

The more important product work is the operator surface: a simpler TUI, durable
coding-run artifacts, permission gates, and first-class local/remote sandbox
execution for software agents.

## Verification Gates

The current Rust workspace should stay green under:

```bash
cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo check -p agentic-harness --no-default-features
```

Documentation changes should also pass:

```bash
npx --yes markdownlint-cli2 '**/*.md'
npx --yes prettier --check --prose-wrap always '**/*.md'
```
