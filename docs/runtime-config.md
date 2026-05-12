# Runtime Config

Agentic Harness can load model and provider defaults from JSON instead of
requiring every app to wire them in Rust code.

Use runtime config for provider defaults, API-key environment variable names,
and OpenAI-compatible model registration. Keep secrets in environment variables;
do not write literal production API keys into repository config.

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

If the file is in one of the workspace convention paths,
`load_workspace_context` will discover it automatically:

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

- `defaultModel`: agent-wide default model in `provider/model-id` form.
- `openaiCompatibleModels`: models to register with the built-in
  OpenAI-compatible HTTP client.
- `providers`: provider runtime settings keyed by provider name.

Provider settings support:

- `baseUrl`: provider or gateway base URL. The built-in client appends
  `/chat/completions`.
- `apiKey`: literal API key, useful for gateway dummy keys.
- `apiKeyEnv`: environment variable to read at request time.
- `headers`: extra provider headers.

The runtime still accepts manual Rust registration with `AgentApp::model`,
`default_model`, and `providers`; file config is a convenience layer over the
same model/provider runtime.

## Precedence

When a prompt is executed, model selection follows this order:

1. `PromptOptions::model(...)`
2. the selected role's `model` metadata, when present
3. `defaultModel` from runtime config

The selected model id must already be registered, either by runtime config or
manual Rust registration with `AgentApp::model(...)`.

Provider settings are merged by provider name. A later runtime config call can
override fields for the same provider.

## OpenAI-Compatible Models

`openaiCompatibleModels` registers model ids that use the built-in
chat-completions client. The provider part of the model id selects the provider
settings:

```json
{
  "defaultModel": "gateway/gpt-5.5",
  "openaiCompatibleModels": ["gateway/gpt-5.5"],
  "providers": {
    "gateway": {
      "baseUrl": "https://gateway.example/v1",
      "apiKeyEnv": "GATEWAY_API_KEY"
    }
  }
}
```

In non-native adapters, register a model with
`OpenAiCompatibleModel::with_transport(...)` so platform HTTP (for example
Worker `fetch`) handles the request.
