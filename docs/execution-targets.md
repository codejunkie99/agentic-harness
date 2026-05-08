# Execution Targets

Agentic Harness is the native Rust runtime for software agents. Its primary
job is to run agents that can inspect repositories, edit files, call tools, run
tests, and preserve task history. Those jobs need a real execution environment,
so deployment targets and coding targets are intentionally separate.

## Default Product Path

The default coding-agent path should be:

```text
Agentic Stack TUI or host adapter
  -> Agentic Harness runtime
  -> local checkout, CI runner, or remote Linux sandbox
```

The user-facing product should be simple, for example a top-level `agentic` TUI
with actions like Code, Review, Test, Triage, and Release. Long CLI commands
remain useful for CI and other software agents, but they should not be the main
human interface.

Agentic Harness stays below that interface as the execution engine. It provides
agents, sessions, model calls, tools, file/search/shell helpers, MCP, structured
results, and HTTP/SSE route handling.

## Target Roles

| Target | Role | Why |
| --- | --- | --- |
| Local checkout | Default development target | Fast, transparent, best for interactive coding and TUI workflows. |
| CI runner | Automation target | Good for issue triage, PR review, test reproduction, and release jobs. |
| Vercel Sandbox | Hosted coding target | Provides a real Linux-style environment for filesystem, shell, dependency install, tests, and isolated code changes. |
| Daytona/E2B/other remote sandbox | Hosted coding target | Same role as Vercel Sandbox: isolate shell and filesystem work from the host. |
| Native HTTP server | Self-hosted API target | Useful when another app or service invokes agents over HTTP/SSE. |
| Node host package | Node-oriented host target | Starts the native Rust server through `server.mjs` on platforms that expect a Node entrypoint. |
| Cloudflare Workers | Edge/control target | Good for webhooks, route metadata, small control endpoints, and Durable Object routing; not the default place to run coding work. |

## Why Cloudflare Is Not Central

Cloudflare Workers are useful for lightweight edge logic, but coding agents need
capabilities Workers do not naturally provide:

- long-running shell commands,
- compilers and test runners,
- package manager installs,
- real repository filesystems,
- browser or build tooling,
- isolated but stateful workspaces.

For that reason, Agentic Harness keeps `--target cloudflare` as build-only
boundary output. It can generate Worker and Durable Object routing artifacts for
webhook/control-plane use, but serious coding work should be delegated to a
native process, CI runner, or remote sandbox.

## Remote Sandbox Contract

Remote coding environments should implement the `SessionEnv` contract. The
simplest portable route is `HttpSessionEnv`, where Agentic Harness sends JSON
operations to a trusted sandbox service:

- `exec` for shell commands with cwd/env/timeout,
- `read`, `write`, `edit`, `grep`, and `glob` behavior through file helpers,
- `stat`, `readdir`, `exists`, `mkdir`, and `rm` for filesystem management.

Provider-specific connectors should be ordinary Rust modules in the user's
project. The connector owns provider authentication and lifecycle, then exposes
the environment to an agent with:

```rust
let session = ctx.session_with_id_and_env("project", sandbox_env);
```

That keeps prompts and model context separate from provider credentials.

When a provider can expose the documented HTTP protocol directly, use
`SandboxConnector::vercel`, `SandboxConnector::daytona`, or
`SandboxConnector::e2b` to construct the `HttpSessionEnv` bridge without writing
provider boilerplate.

## Agentic Stack Integration

Agentic Stack should own the operator surface:

- TUI dashboard,
- policy and permission prompts,
- host adapters for Codex, Claude Code, OpenCode, Cursor, and CI,
- task templates,
- run history,
- local or remote sandbox selection.

Agentic Harness should own the runtime:

- Rust agent definitions,
- sessions and compaction,
- model/provider configuration,
- tools and MCP,
- file/search/shell abstractions,
- native build/run/dev/serve behavior,
- platform-neutral HTTP/SSE entrypoints.

This split keeps the runtime reusable while allowing Agentic Stack to become
the simple product surface people actually operate.

## Practical Roadmap

1. Keep the current native Rust runtime green and complete.
2. Add a simple `agentic` TUI in Agentic Stack that calls Agentic Harness under
   the hood.
3. Make Code, Review, Test, Triage, and Release the default TUI actions.
4. Treat local checkout as the first execution target.
5. Treat Vercel Sandbox and other remote sandboxes as first-class hosted coding
   targets through `SessionEnv`.
6. Keep Cloudflare as optional webhook/control-plane output, not the default
   coding runtime.
