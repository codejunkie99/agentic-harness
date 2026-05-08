# Runtime Config

Agentic Harness can load model and provider defaults from JSON instead of
requiring every app to wire them in Rust code.

```json
{
  "defaultModel": "openai/gpt-5.5",
  "openaiCompatibleModels": ["openai/gpt-5.5"],
  "providers": {
    "openai": {
      "baseUrl": "https://api.openai.com/v1",
      "apiKeyEnv": "OPENAI_API_KEY",
      "headers": {
        "X-Gateway": "example"
      }
    }
  }
}
```

If the file is in one of the workspace convention paths, `load_workspace_context`
will discover it automatically:

- `agentic-harness.json`
- `.agentic-harness/config.json`

```rust
use agentic_harness::{AgentApp, AgenticHarnessError};

fn app() -> Result<AgentApp, AgenticHarnessError> {
    AgentApp::new()
        .with_workspace(".")
        .load_workspace_context()
}
```

Load an explicit file path when the config lives elsewhere:

```rust
use agentic_harness::{run_cli, AgentApp, AgenticHarnessError};

fn app() -> Result<AgentApp, AgenticHarnessError> {
    AgentApp::new()
        .load_runtime_config("agentic-harness.json")?
        .load_workspace_context()
}
```

## Fields

| Field | Purpose |
| --- | --- |
| `defaultModel` | Agent-wide default model in `provider/model-id` form. |
| `openaiCompatibleModels` | Models to register with the built-in OpenAI-compatible HTTP client. |
| `providers` | Provider runtime settings keyed by provider name. |

Provider settings support:

| Field | Purpose |
| --- | --- |
| `baseUrl` | Provider or gateway base URL. The built-in client appends `/chat/completions`. |
| `apiKey` | Literal API key, useful for gateway dummy keys. |
| `apiKeyEnv` | Environment variable to read at request time. |
| `headers` | Extra provider headers. |

The runtime still accepts manual Rust registration with `AgentApp::model`,
`default_model`, and `providers`; file config is a convenience layer over the
same model/provider runtime.
