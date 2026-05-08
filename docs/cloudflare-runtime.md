# Cloudflare Runtime Boundary

Agentic Harness currently builds and runs native Rust server artifacts. The
native runtime uses operating-system primitives that Cloudflare Workers do not
provide: TCP listeners, process spawning, host filesystem access, and blocking
HTTP calls.

Cloudflare is therefore an optional edge-control target, not the primary
execution environment for coding agents. Coding agents should run against a
local checkout, CI runner, or remote Linux sandbox such as Vercel Sandbox,
Daytona, or E2B, where shell commands, compilers, package installs, tests, and
filesystem edits are natural operations. See
[`execution-targets.md`](execution-targets.md) for the overall target model.

For that reason, `--target cloudflare` is build-only. It emits Worker boundary
artifacts, but `run` and `dev` still reject the target. A deployable Cloudflare
target must be a separate Worker-compatible runtime, not a proxy to a native
process.

The SDK now has an initial core/native split: `agentic-harness` defaults to the
`native` feature, while `cargo check -p agentic-harness --no-default-features`
keeps the provider-neutral core types buildable without the native blocking HTTP
client dependency. Native filesystem, process, TCP serving, workspace discovery,
file-backed session storage, and embedded CLI entrypoints are also gated behind
the `native` feature.

The core session layer now accepts any implementation of `SessionStore`, so a
Durable Object or Workers KV storage adapter can persist `SessionData` without
pulling in the native file-backed store. Worker runtimes should use the
`try_session...` constructors so storage load failures return request errors
instead of silently starting empty histories. That is still only an adapter
boundary; it is not a deployable Worker runtime by itself.

For sandbox-style command and filesystem operations, `HttpSessionEnv` also works
without the native feature when constructed with
`HttpSessionEnv::with_transport(...)`. A Worker adapter can implement
`HttpSessionTransport` by forwarding the JSON protocol through Worker `fetch` or
another platform capability.

The OpenAI-compatible model client is also split at a transport boundary.
Native binaries use the built-in `reqwest` transport, while Worker/WASM adapters
can construct `OpenAiCompatibleModel::with_transport(...)` with an
`HttpModelTransport` implementation that forwards requests through platform
HTTP, such as Worker `fetch`.

The core HTTP layer also exposes `HttpRequest` and `AgentApp::handle_request`.
That lets a Worker entrypoint pass method, path, headers, and body into the core
router without using native TCP serving. The core router supports
`Accept: text/event-stream` on `/agents/<name>/<id>` for SSE responses, matching
the Worker request shape expected by a future Cloudflare runtime.

For live streaming, the core exposes `AgentApp::invoke_with_event_sink`. A Worker
adapter can connect that sink to a `TransformStream` writer so text/tool/result
events are emitted during handler execution instead of only after the handler
returns. `RuntimeEvent::to_sse_frame` provides the shared SSE frame encoding for
that stream.

For WASM app adapters, the core also exposes `WorkerAppRequest`,
`WorkerAppResponse`, and `AgentApp::handle_worker_app_request`. The request
shape is `{ agentName, id, payload }`; the response shape carries status, result,
error envelope, and runtime events. The generated default WASM adapter unwraps
that response, throws typed errors, and returns the result to the Worker runtime
shim.
Rust WASM crates can use `agentic_harness_worker_app!(app_fn)` to export the ABI
functions expected by that generated adapter.

The core can now derive Cloudflare routing metadata with
`AgentApp::cloudflare_worker_manifest`. It maps each webhook agent to the
Durable Object binding/class name a Wrangler output would need and rejects agent
names that are not lower-kebab-case, matching the routing constraint of the
previous Worker generator. `CloudflareWorkerManifest::wrangler_config` then
emits the corresponding Wrangler JSON fragment for Durable Object bindings and
SQLite migrations. `CloudflareWorkerManifest::worker_entrypoint` emits a
non-proxy Worker module that handles `/health`, `/agents`, and agent routes by
dispatching to the generated Durable Object bindings.

The native CLI can retrieve that metadata from a Rust app with
`agentic-harness manifest --cloudflare --workspace <path>`. It also uses the
same metadata in `agentic-harness build --target cloudflare` to write:

- `dist/_entry.js`: Worker entrypoint and Durable Object routing module.
- `dist/wrangler.jsonc`: Worker main module, compatibility date, Durable
  Object binding, and SQLite migration config.
- `dist/cloudflare-manifest.json`: raw Worker routing metadata.
- `dist/agentic_harness_worker.js`: Worker-side JSON parsing, synchronous
  responses, SSE responses, webhook acceptance, and Durable Object SQL-backed
  session-store plumbing.
- `dist/agentic_harness_app.js`: the remaining app-adapter boundary. It returns
  `handler_not_linked` until replaced by a Rust/WASM adapter for the workspace.
- `dist/agentic_harness_app.d.ts`: the stable adapter contract for
  `AgenticHarnessWorkerContext`, `AgenticHarnessSessionStore`, event emission,
  initialization, and invocation.

When a Worker-compatible app adapter already exists, pass
`--worker-app <path>` to `agentic-harness build --target cloudflare`. The CLI
copies that file to `dist/agentic_harness_app.js` instead of writing the
`handler_not_linked` stub. The adapter must export:

- `initAgenticHarnessApp()`: optional async initialization hook.
- `invokeAgenticHarnessAgent(context)`: async handler called with `{ request,
  state, env, agentName, id, payload, sessionStore, emit }`.

Pass `--worker-wasm <path>` to copy the adapter's companion module to
`dist/agentic_harness_app.wasm`. If no `--worker-app` is provided, the CLI
generates a default JSON ABI adapter. That adapter expects exported
`memory`, `agentic_harness_alloc(len)`, `agentic_harness_invoke(ptr, len)`,
`agentic_harness_last_result_len()`, and optional `agentic_harness_init()`.
The SDK macro also exports `agentic_harness_dealloc(ptr, len)` for callers that
need to free input buffers after invocation.
Pass `--worker-wasm-crate <path>` to compile that module from a Rust crate with
`cargo build --release --target wasm32-unknown-unknown --no-default-features`
and package the resulting `.wasm`.

This is a non-proxy build boundary, not a complete Cloudflare deployment path.

## Intended Role

Use the Cloudflare output for lightweight webhook/control-plane cases:

- accept webhooks and route them to durable per-agent ids,
- expose `/health`, `/agents`, and agent route metadata at the edge,
- persist small session/control records in Durable Object storage,
- forward work to a Worker-compatible adapter when one exists.

Do not use it as the default place to perform coding work. For that, use native
execution or a remote sandbox connector.

## Required Runtime Shape

A real Cloudflare target needs these pieces:

- Worker-compatible Rust/WASM handler adapter behind `agentic_harness_app.js`.
- Worker-specific wiring that passes model and sandbox transports into the
  handler adapter, such as an in-memory filesystem, Cloudflare storage-backed
  filesystem, or explicit remote sandbox connector.
- Replacement of the generated `handler_not_linked` app adapter with real handler
  execution, without starting or proxying to a native server.

## Non-Goals

- Do not tunnel requests to the native binary.
- Do not emit a proxy target.
- Do not claim Cloudflare compatibility while the runtime still depends on
  `std::net`, `std::process`, host filesystem access, or blocking HTTP.

## Current Supported Targets

- `native`: builds and runs the native Rust server artifact.
- `node`: Node host package that launches the native Rust server artifact with
  `node server.mjs`.
- `cloudflare`: build-only Worker boundary output. It does not run or develop
  locally through the native CLI, and the generated app-adapter contract must be
  replaced by a Worker-compatible handler runtime before deployment.

Full Cloudflare support should only be claimed when the Worker-compatible
runtime above exists and has its own tests.
