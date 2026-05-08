# Sandbox Connectors

Agentic Harness has two connector paths.

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
`SandboxConnector::daytona`, and `SandboxConnector::e2b`. They tag outbound
HTTP requests with the provider name and keep provider credentials in trusted
Rust code.

Use `agentic-harness add <provider> --print` when the provider needs its own
Rust module around an SDK or CLI:

```bash
agentic-harness add daytona --print
agentic-harness add e2b --print
agentic-harness add vercel --print
```

Those instructions produce a project-local `SessionEnv` implementation for the
provider.
