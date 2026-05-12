# Agentic Harness Docs

Long-form documentation for [Agentic Harness](../README.md). Use the top-level
[README](../README.md) for install, examples, and the common command path. Use
this folder for architecture, runtime protocols, deployment boundaries, feature
status, and release checks.

## Recommended Reading Order

1. [Execution Targets](execution-targets.md): choose local, CI, remote sandbox,
   Node host, or Cloudflare boundary output.
2. [Runtime Config](runtime-config.md): configure models and providers.
3. [HTTP SessionEnv Protocol](http-session-env.md): connect remote sandboxes.
4. [Feature Status](feature-status.md): verify what is implemented today.

## Architecture

- **[Execution Targets](execution-targets.md)** — local development, CI, remote
  Linux sandboxes (Vercel Sandbox, Daytona, E2B), and Cloudflare edge: what runs
  where, and why.
- **[Runtime Config](runtime-config.md)** — provider defaults, env-key
  resolution, OpenAI-compatible model registration, role/call/default
  precedence, and `runtime.json` discovery rules.
- **[HTTP SessionEnv Protocol](http-session-env.md)** — the JSON-over-HTTP wire
  format that lets a Rust agent run shell, file, and env operations inside any
  remote sandbox that speaks the protocol.
- **[Sandbox Connectors](connectors.md)** — when to use the built-in
  `SandboxConnector` helpers and when to generate a project-local connector.
- **[Virtual Sandbox](virtual-sandbox.md)** — hostless in-memory filesystem and
  small shell subset for tests, support agents, and Worker-style adapters.
- **[Cloudflare Runtime](cloudflare-runtime.md)** — Worker boundary build
  pipeline, Durable Object bindings, the WASM JSON ABI, and the `--worker-app` /
  `--worker-wasm` / `--worker-wasm-crate` build options.

## Deployment Guides

- **[Node Hosts](deploy-node.md)** — package a native Rust agent behind
  `node server.mjs` for hosts that require a Node entrypoint.
- **[Cloudflare Workers](deploy-cloudflare.md)** — generate Worker boundary
  files and link a Worker-compatible app adapter.
- **[GitHub Actions](deploy-github-actions.md)** — run agents in checked-out
  repositories for issue triage, PR review, and release jobs.
- **[GitLab CI](deploy-gitlab-ci.md)** — run agents in GitLab runners with
  explicit CI credentials.

## Status & Releases

- **[Feature Status](feature-status.md)** — capability matrix with code, tests,
  and docs evidence for every major surface.
- **[Roadmap](immediate-goals.md)** — the next product slice and explicit
  non-goals.
- **[Release Smoke Test](release-smoke-test.md)** — clean-machine pre-publish
  checklist (`smoke --json`, `release-check --json`, install script, Homebrew
  formula, changelog).
- **[Flue Migration](flue-migration.md)** — map older TypeScript Flue concepts
  to native Rust Agentic Harness concepts.
