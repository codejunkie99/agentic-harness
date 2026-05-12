# Sandbox Connectors

Agentic Harness has two connector paths.

Use connectors when an agent should run shell and filesystem work somewhere
other than the local checkout. Keep provider authentication in Rust code,
environment variables, or provider control planes; do not place credentials in
prompts or workspace instructions.

## Built-In HTTP Connectors

Use `SandboxConnector` when a hosted provider exposes the documented
`HttpSessionEnv` protocol:

```rust
use agentic_harness::{SandboxConnector, ShellOptions, SessionEnv};

let env = SandboxConnector::vercel("https://sandbox.example/session", "/workspace/project")
    .header("Authorization", format!("Bearer {}", token))
    .into_session_env();

let output = env.exec("cargo test", ShellOptions::new())?;
```

Built-in provider labels are `SandboxConnector::vercel`,
`SandboxConnector::daytona`, and `SandboxConnector::e2b`. They tag outbound HTTP
requests with the provider name and keep provider credentials in trusted Rust
code.

## Generated Project Connectors

Use `agentic-harness add <provider> --print` when the provider needs its own
Rust module around an SDK or CLI:

```bash
agentic-harness add daytona --print
agentic-harness add e2b --print
agentic-harness add vercel --print
```

Those instructions produce a project-local `SessionEnv` implementation for the
provider.

## Which Path To Use

- Use `SandboxConnector::<provider>` when the provider already speaks the
  Agentic Harness HTTP protocol.
- Use `agentic-harness add <provider> --print` when the provider has an SDK,
  CLI, or custom API that needs a Rust adapter.
- Use `agentic-harness add <url-or-path> --category sandbox --print` when you
  have provider docs and need a coding agent to build a connector from them.

After binding a connector with `ctx.session_with_id_and_env(...)`, normal
session helpers such as `session.shell`, `session.read`, `session.write`,
`session.grep`, and `session.glob` execute in the connector-backed environment.
