# Agentic Harness Docs

Long-form documentation for [Agentic Harness](../README.md). For overview, install, and runnable examples, see the top-level [README](../README.md). This folder covers architecture, protocols, and operational concerns that don't fit on the front page.

## Architecture

- **[Execution Targets](execution-targets.md)** — local development, CI, remote Linux sandboxes (Vercel Sandbox, Daytona, E2B), and Cloudflare edge — what runs where, and why.
- **[Runtime Config](runtime-config.md)** — provider defaults, env-key resolution, OpenAI-compatible model registration, role/call/default precedence, and `runtime.json` discovery rules.
- **[HTTP SessionEnv Protocol](http-session-env.md)** — the JSON-over-HTTP wire format that lets a Rust agent run shell, file, and env operations inside any remote sandbox that speaks the protocol.
- **[Cloudflare Runtime](cloudflare-runtime.md)** — Worker boundary build pipeline, Durable Object bindings, the WASM JSON ABI, and the `--worker-app` / `--worker-wasm` / `--worker-wasm-crate` build options.

## Status & Releases

- **[Feature Status](feature-status.md)** — capability matrix with code, tests, and docs evidence for every major surface.
- **[Roadmap](immediate-goals.md)** — the next product slice and explicit non-goals.
- **[Release Smoke Test](release-smoke-test.md)** — clean-machine pre-publish checklist (`smoke --json`, `release-check --json`, install script, Homebrew formula, changelog).
