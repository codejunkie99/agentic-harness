# Flue Migration Guide

This is the compatibility map for teams moving TypeScript agents to native
Rust Agentic Harness agents.

| Flue concept | Agentic Harness replacement |
| --- | --- |
| `FlueContext` | `AgentContext` |
| `init({ model, sandbox })` | `AgentApp::model(...)` plus `ctx.session_with_env(...)` |
| `session.prompt()` | `Session::prompt_with_options(...)` |
| `session.skill()` | `Session::skill(...)` |
| `session.task()` | `Session::task(...)` |
| `defineCommand()` | `CommandDef` and `AgentContext::command(...)` |
| `getVirtualSandbox()` | `VirtualSessionEnv` or a custom `SessionEnv` |
| `connectMcpServer()` | `connect_mcp_server(...)` |
| `flue add daytona` | `agentic-harness add daytona --print` |
| `flue dev/run/build` | `agentic-harness dev/run/build` |

Minimal webhook rewrite:

```rust
use agentic_harness::prelude::*;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
struct Payload {
    text: String,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
        let payload: Payload = ctx.payload()?;
        Ok(json!({ "message": payload.text }))
    })))
}
```

Migration order:

1. Move `AGENTS.md`, roles, and skills into the Rust workspace.
2. Rewrite each `.flue/agents/*.ts` handler as a Rust `AgentDefinition`.
3. Replace TypeScript schemas with Rust `Deserialize` payload/result structs.
4. Replace sandbox imports with `VirtualSessionEnv`, `HttpSessionEnv`, or a
   provider `SessionEnv` connector.
5. Run `agentic-harness doctor --workspace .`, then `agentic-harness run`.
