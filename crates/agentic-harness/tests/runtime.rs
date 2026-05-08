#![cfg(feature = "native")]

use agentic_harness::{
    mcp_tools_from_client, AgentApp, AgentContext, AgentDefinition, AgentRuntimeConfig,
    AgenticHarnessError, CloudflareDurableObjectBinding, CommandDef, CompactionOptions,
    CompactionSettings, FileStat, HttpRequest, HttpSessionEnv, HttpSessionTransport,
    HttpSessionTransportResponse, McpClient, McpTool, McpTransport, MemorySessionEnv, ModelClient,
    ModelMessage, ModelRequest, OpenAiCompatibleModel, PromptOptions, PromptResponse,
    ProviderSettings, ProvidersConfig, ReadOptions, RuntimeEvent, SessionEnv, ShellOptions,
    ShellOutput, ToolCall, ToolDef,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct NamePayload {
    name: String,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct StructuredResult {
    answer: String,
    confidence: u8,
}

struct NamedModel(&'static str);

impl ModelClient for NamedModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        Ok(PromptResponse::text(format!(
            "{}:{}",
            self.0,
            request.system.contains("Use warm language.")
        )))
    }
}

struct ToolInspectModel;

impl ModelClient for ToolInspectModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        Ok(PromptResponse::text(serde_json::to_string(&request.tools)?))
    }
}

struct ToolCallingModel {
    calls: Arc<Mutex<usize>>,
}

impl ModelClient for ToolCallingModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            assert!(request.tools.iter().any(|tool| tool.name == "lookup"));
            return Ok(PromptResponse::with_tool_calls(
                "",
                vec![ToolCall {
                    id: "call-1".to_string(),
                    name: "lookup".to_string(),
                    arguments: json!({ "id": "42" }),
                }],
            ));
        }

        let tool_result = request
            .messages
            .iter()
            .find(|message| message.role == "tool")
            .map(|message| message.content.as_str())
            .unwrap_or("");
        Ok(PromptResponse::text(format!("final saw {tool_result}")))
    }
}

struct BuiltinToolModel {
    calls: Arc<Mutex<usize>>,
}

#[test]
fn sdk_prelude_exports_common_agent_authoring_types() {
    use agentic_harness::prelude::*;

    let app = AgentApp::new().agent(AgentDefinition::cli_only("ping", |ctx: AgentContext| {
        let _prompt_options = PromptOptions::new().role("coder").model("local/test");
        let _shell_options = ShellOptions::new()
            .cwd(".")
            .timeout(Duration::from_millis(100));
        let _memory_env = MemorySessionEnv::new(".");
        Ok(json!({ "id": ctx.id(), "ok": true }))
    }));
    let _run: fn(AgentApp) -> Result<i32, AgenticHarnessError> = run_cli;

    assert_eq!(app.manifest().agents[0].name, "ping");
}

impl ModelClient for BuiltinToolModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let mut calls = self.calls.lock().unwrap();
        *calls += 1;
        if *calls == 1 {
            let tool_names = request
                .tools
                .iter()
                .map(|tool| tool.name.as_str())
                .collect::<Vec<_>>();
            assert!(tool_names.contains(&"read"));
            assert!(tool_names.contains(&"bash"));
            assert!(tool_names.contains(&"grep"));
            assert!(tool_names.contains(&"glob"));
            assert!(tool_names.contains(&"task"));
            return Ok(PromptResponse::with_tool_calls(
                "",
                vec![ToolCall {
                    id: "builtin-read-1".to_string(),
                    name: "read".to_string(),
                    arguments: json!({
                        "path": "notes.txt",
                        "maxBytes": 5
                    }),
                }],
            ));
        }

        let tool_result = request
            .messages
            .iter()
            .find(|message| message.role == "tool")
            .map(|message| message.content.as_str())
            .unwrap_or("");
        assert!(tool_result.contains("alpha"));
        assert!(tool_result.contains("truncated"));
        Ok(PromptResponse::text("builtin final"))
    }
}

#[derive(Clone)]
struct FakeMcpClient {
    calls: Arc<Mutex<Vec<(String, Value)>>>,
}

impl McpClient for FakeMcpClient {
    fn list_tools(&self) -> Result<Vec<McpTool>, AgenticHarnessError> {
        Ok(vec![McpTool {
            name: "search.docs".to_string(),
            title: Some("Search Docs".to_string()),
            description: Some("Search project docs".to_string()),
            input_schema: json!({
                "properties": { "q": { "type": "string" } },
                "required": ["q"]
            }),
        }])
    }

    fn call_tool(&self, name: &str, args: Value) -> Result<Value, AgenticHarnessError> {
        self.calls
            .lock()
            .unwrap()
            .push((name.to_string(), args.clone()));
        Ok(json!({
            "content": [{ "type": "text", "text": format!("found {}", args["q"].as_str().unwrap()) }]
        }))
    }
}

#[derive(Clone)]
struct RecordingEnv {
    cwd: PathBuf,
    files: Arc<Mutex<BTreeMap<String, String>>>,
    execs: Arc<Mutex<Vec<(String, ShellOptions)>>>,
}

impl RecordingEnv {
    fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            files: Arc::new(Mutex::new(BTreeMap::new())),
            execs: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl SessionEnv for RecordingEnv {
    fn exec(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        self.execs
            .lock()
            .unwrap()
            .push((command.to_string(), options.clone()));
        Ok(ShellOutput {
            stdout: format!(
                "{}:{}:{}",
                command,
                options.cwd.as_ref().unwrap_or(&self.cwd).to_string_lossy(),
                options.env.get("TOKEN").cloned().unwrap_or_default()
            ),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> {
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| AgenticHarnessError::Handler {
                message: format!("missing fake file {path}"),
            })
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> {
        self.files.lock().unwrap().insert(
            path.to_string(),
            String::from_utf8(content.to_vec()).unwrap(),
        );
        Ok(())
    }

    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        let files = self.files.lock().unwrap();
        Ok(FileStat {
            is_file: files.contains_key(path),
            is_directory: false,
            is_symbolic_link: false,
            size: files.get(path).map(|content| content.len()).unwrap_or(0) as u64,
            modified_unix_ms: None,
        })
    }

    fn readdir(&self, _path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        Ok(self.files.lock().unwrap().keys().cloned().collect())
    }

    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        Ok(self.files.lock().unwrap().contains_key(path))
    }

    fn mkdir(&self, _path: &str) -> Result<(), AgenticHarnessError> {
        Ok(())
    }

    fn rm(&self, path: &str, _recursive: bool) -> Result<(), AgenticHarnessError> {
        self.files.lock().unwrap().remove(path);
        Ok(())
    }

    fn cwd(&self) -> &Path {
        &self.cwd
    }

    fn resolve_path(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }
}

#[derive(Clone)]
struct RecordingHttpSessionTransport {
    requests: Arc<Mutex<Vec<(String, BTreeMap<String, String>, Value)>>>,
}

impl RecordingHttpSessionTransport {
    fn new() -> Self {
        Self {
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn requests(&self) -> Vec<(String, BTreeMap<String, String>, Value)> {
        self.requests.lock().unwrap().clone()
    }
}

impl HttpSessionTransport for RecordingHttpSessionTransport {
    fn post_json(
        &self,
        url: &str,
        headers: &BTreeMap<String, String>,
        body: &Value,
    ) -> Result<HttpSessionTransportResponse, AgenticHarnessError> {
        self.requests
            .lock()
            .unwrap()
            .push((url.to_string(), headers.clone(), body.clone()));
        let (status, response) = http_session_env_test_response(body);
        Ok(HttpSessionTransportResponse {
            status,
            body: serde_json::to_string(&response).unwrap(),
        })
    }
}

fn http_session_env_test_response(request: &Value) -> (u16, Value) {
    match request["op"].as_str().unwrap() {
        "exec" => (
            200,
            json!({
                "stdout": format!("remote:{}", request["command"].as_str().unwrap()),
                "stderr": "",
                "exitCode": 0
            }),
        ),
        "write" => (200, json!({ "ok": true })),
        "read" => {
            if request["path"] == "missing.txt" {
                (
                    500,
                    json!({ "error": { "message": "missing remote file" } }),
                )
            } else {
                (
                    200,
                    json!(format!("remote-file:{}", request["path"].as_str().unwrap())),
                )
            }
        }
        "stat" => (
            200,
            json!({
                "isFile": true,
                "isDirectory": false,
                "isSymbolicLink": false,
                "size": 18,
                "modifiedUnixMs": 42
            }),
        ),
        "readdir" => (200, json!(["notes.txt", "src"])),
        "exists" => (200, json!(true)),
        "mkdir" => (200, json!({ "ok": true })),
        "rm" => (200, json!({ "ok": true })),
        other => (
            500,
            json!({ "error": { "message": format!("unexpected op {other}") } }),
        ),
    }
}

#[test]
fn manifest_lists_registered_agents_in_stable_order() {
    let app = AgentApp::new()
        .agent(AgentDefinition::webhook("zeta", |_| {
            Ok(json!({ "ok": true }))
        }))
        .agent(AgentDefinition::cli_only("alpha", |_| {
            Ok(json!({ "ok": true }))
        }));

    let manifest = app.manifest();

    assert_eq!(
        serde_json::to_value(manifest).unwrap(),
        json!({
            "agents": [
                { "name": "alpha", "triggers": { "webhook": false } },
                { "name": "zeta", "triggers": { "webhook": true } }
            ]
        })
    );
}

#[test]
fn cloudflare_worker_manifest_maps_webhook_agents_to_durable_object_bindings() {
    let app = AgentApp::new()
        .agent(AgentDefinition::webhook("hello-world", |_| {
            Ok(json!({ "ok": true }))
        }))
        .agent(AgentDefinition::cli_only("triage", |_| {
            Ok(json!({ "ok": true }))
        }));

    let manifest = app.cloudflare_worker_manifest().unwrap();

    assert_eq!(
        manifest.durable_objects,
        vec![CloudflareDurableObjectBinding {
            agent_name: "hello-world".to_string(),
            binding_name: "HelloWorld".to_string(),
            class_name: "HelloWorld".to_string(),
        }]
    );
    assert_eq!(manifest.webhook_agent_names, vec!["hello-world"]);
}

#[test]
fn cloudflare_worker_manifest_rejects_agent_names_that_cannot_route_to_bindings() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("Hello_World", |_| {
        Ok(json!({ "ok": true }))
    }));

    let err = app.cloudflare_worker_manifest().unwrap_err();

    assert_eq!(err.error_type(), "invalid_deployment");
    assert!(err.to_string().contains("lower-kebab-case"));
}

#[test]
fn cloudflare_worker_manifest_generates_wrangler_config_fragment() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("hello-world", |_| {
        Ok(json!({ "ok": true }))
    }));

    let config = app
        .cloudflare_worker_manifest()
        .unwrap()
        .wrangler_config("demo-agent", "_entry.js");

    assert_eq!(
        config,
        json!({
            "$schema": "https://workers.cloudflare.com/schema/wrangler.json",
            "name": "demo-agent",
            "main": "_entry.js",
            "durable_objects": {
                "bindings": [
                    {
                        "name": "HelloWorld",
                        "class_name": "HelloWorld"
                    }
                ]
            },
            "migrations": [
                {
                    "tag": "agentic-harness-HelloWorld",
                    "new_sqlite_classes": ["HelloWorld"]
                }
            ]
        })
    );
}

#[test]
fn cloudflare_worker_manifest_generates_non_proxy_worker_entrypoint() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("hello-world", |_| {
        Ok(json!({ "ok": true }))
    }));

    let source = app
        .cloudflare_worker_manifest()
        .unwrap()
        .worker_entrypoint("./agentic_harness_worker.js");

    assert!(source.contains(
        "import initWorker, { handleAgenticHarnessDurableObject } from './agentic_harness_worker.js';"
    ));
    assert!(source.contains("const WEBHOOK_AGENT_NAMES = new Set([\"hello-world\"]);"));
    assert!(source.contains("const DURABLE_OBJECT_BINDINGS = {\"hello-world\":\"HelloWorld\"};"));
    assert!(source.contains("export class HelloWorld"));
    assert!(source.contains("static agentName = \"hello-world\";"));
    assert!(source.contains("stub.fetch(request);"));
    assert!(!source.contains("127.0.0.1"));
    assert!(!source.contains("localhost"));
}

#[test]
fn invoke_passes_id_payload_workspace_and_shell_to_agent() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("AGENTS.md"), "native rust context").unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .agent(AgentDefinition::webhook("hello", |ctx: AgentContext| {
            let payload: NamePayload = ctx.payload()?;
            let shell = ctx.shell("cat AGENTS.md")?;
            Ok(json!({
                "id": ctx.id(),
                "message": format!("Hello, {}!", payload.name),
                "context": shell.stdout.trim(),
                "exitCode": shell.exit_code
            }))
        }));

    let result = app
        .invoke("hello", "request-7", json!({ "name": "Ada" }))
        .unwrap();

    assert_eq!(
        result,
        json!({
            "id": "request-7",
            "message": "Hello, Ada!",
            "context": "native rust context",
            "exitCode": 0
        })
    );
}

#[test]
fn context_exposes_native_file_search_and_edit_tools() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/one.txt"), "alpha\nbeta\n").unwrap();
    fs::write(temp.path().join("src/two.txt"), "beta\ngamma\n").unwrap();

    let app = AgentApp::new()
		.with_workspace(temp.path())
		.agent(AgentDefinition::webhook("tools", |ctx: AgentContext| {
			let before = ctx.read("src/one.txt")?;
			ctx.write("src/new.txt", "created")?;
			ctx.edit("src/one.txt", "alpha", "delta")?;
			let after = ctx.read("src/one.txt")?;
			let grep = ctx.grep("beta")?;
			let glob = ctx.glob("src/*.txt")?;
			Ok(json!({
				"before": before,
				"after": after,
				"new": ctx.read("src/new.txt")?,
				"grep": grep.into_iter().map(|m| format!("{}:{}", m.path, m.line)).collect::<Vec<_>>(),
				"glob": glob
			}))
		}));

    let result = app.invoke("tools", "tools-1", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "before": "alpha\nbeta\n",
            "after": "delta\nbeta\n",
            "new": "created",
            "grep": ["src/one.txt:2", "src/two.txt:1"],
            "glob": ["src/new.txt", "src/one.txt", "src/two.txt"]
        })
    );
}

#[test]
fn read_with_options_returns_bounded_content_and_truncation_metadata() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("notes.txt"), "alpha\nbeta\ngamma\n").unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .agent(AgentDefinition::webhook("reader", |ctx: AgentContext| {
            let direct =
                ctx.read_with_options("notes.txt", ReadOptions::new().offset(6).max_bytes(4))?;
            let session = ctx
                .session()
                .read_with_options("notes.txt", ReadOptions::new().max_bytes(5))?;
            Ok(json!({
                "direct": direct,
                "session": session,
                "full": ctx.read("notes.txt")?
            }))
        }));

    let result = app.invoke("reader", "read", json!({})).unwrap();

    assert_eq!(result["direct"]["content"], "beta");
    assert_eq!(result["direct"]["startByte"], 6);
    assert_eq!(result["direct"]["bytesRead"], 4);
    assert_eq!(result["direct"]["totalBytes"], 17);
    assert_eq!(result["direct"]["truncated"], true);
    assert_eq!(result["session"]["content"], "alpha");
    assert_eq!(result["session"]["truncated"], true);
    assert_eq!(result["full"], "alpha\nbeta\ngamma\n");
}

#[test]
fn grep_supports_regex_and_glob_supports_question_mark() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("src")).unwrap();
    fs::write(temp.path().join("src/a1.txt"), "alpha-123\nbeta\n").unwrap();
    fs::write(temp.path().join("src/a2.txt"), "alpha-xyz\n").unwrap();
    fs::write(temp.path().join("src/long-name.txt"), "alpha-456\n").unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .agent(AgentDefinition::webhook("search", |ctx| {
            let regex = ctx.grep(r"alpha-\d+")?;
            let glob = ctx.glob("src/a?.txt")?;
            Ok(json!({
                "regex": regex.into_iter().map(|m| format!("{}:{}", m.path, m.line)).collect::<Vec<_>>(),
                "glob": glob
            }))
        }));

    let result = app.invoke("search", "search", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "regex": ["src/a1.txt:1", "src/long-name.txt:1"],
            "glob": ["src/a1.txt", "src/a2.txt"]
        })
    );
}

#[test]
fn workspace_context_matches_claude_and_codex_conventions() {
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join(".agents/skills/review");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(temp.path().join("AGENTS.md"), "AGENTS context").unwrap();
    fs::write(temp.path().join("CLAUDE.md"), "CLAUDE context").unwrap();
    fs::write(temp.path().join("notes.txt"), "visible in listing").unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review code\n---\nReview instructions.\n",
    )
    .unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .load_workspace_context()
        .unwrap()
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let response = session.prompt("inspect context", &SystemPromptModel)?;
            Ok(json!({ "system": response.text }))
        }));

    let result = app.invoke("assistant", "ctx", json!({})).unwrap();
    let system = result["system"].as_str().unwrap();

    assert!(system.contains("AGENTS context"));
    assert!(system.contains("CLAUDE context"));
    assert!(system.contains("## Available Skills"));
    assert!(system.contains("- **review** - Review code"));
    assert!(system.contains("Working directory:"));
    assert!(system.contains("Directory structure:"));
    assert!(system.contains("notes.txt"));
}

#[test]
fn shell_options_scope_cwd_env_timeout_and_file_helpers() {
    let temp = tempfile::tempdir().unwrap();
    let subdir = temp.path().join("subdir");
    fs::create_dir_all(&subdir).unwrap();
    fs::write(subdir.join("item.txt"), "from cwd").unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .agent(AgentDefinition::webhook("tools", |ctx| {
            ctx.mkdir("created/nested")?;
            ctx.write("created/nested/file.txt", "created")?;
            let stat = ctx.stat("created/nested/file.txt")?;
            let entries = ctx.readdir("created/nested")?;
            let exists_before = ctx.exists("created/nested/file.txt")?;

            let scoped = ctx.shell_with_options(
                "printf '%s:%s' \"$AGENTIC_HARNESS_TEST\" \"$(cat item.txt)\"",
                ShellOptions::new()
                    .cwd("subdir")
                    .env("AGENTIC_HARNESS_TEST", "scoped")
                    .timeout(Duration::from_secs(1)),
            )?;

            let timed_out = ctx.shell_with_options(
                "sleep 2",
                ShellOptions::new().timeout(Duration::from_millis(50)),
            )?;

            ctx.rm("created/nested/file.txt", false)?;
            let exists_after = ctx.exists("created/nested/file.txt")?;

            Ok(json!({
                "stat": stat,
                "entries": entries,
                "existsBefore": exists_before,
                "existsAfter": exists_after,
                "scoped": scoped,
                "timedOut": timed_out
            }))
        }));

    let result = app.invoke("tools", "tools", json!({})).unwrap();

    assert_eq!(result["stat"]["isFile"], true);
    assert_eq!(result["stat"]["isDirectory"], false);
    assert_eq!(result["stat"]["size"], 7);
    assert_eq!(result["entries"], json!(["file.txt"]));
    assert_eq!(result["existsBefore"], true);
    assert_eq!(result["existsAfter"], false);
    assert_eq!(result["scoped"]["stdout"], "scoped:from cwd");
    assert_eq!(result["scoped"]["exitCode"], 0);
    assert_ne!(result["timedOut"]["exitCode"], 0);
    assert!(result["timedOut"]["stderr"]
        .as_str()
        .unwrap()
        .contains("timed out"));
}

#[test]
fn sessions_can_run_file_and_shell_helpers_against_custom_session_env() {
    let env = RecordingEnv::new("/sandbox/project");
    let captured_execs = env.execs.clone();
    let app = AgentApp::new().agent(AgentDefinition::webhook("remote", move |ctx| {
        let session = ctx.session_with_id_and_env("remote-env", env.clone());
        session.write("notes.txt", "alpha beta")?;
        session.write("log.txt", "alpha\nomega")?;
        session.edit("notes.txt", "beta", "gamma")?;
        let read = session.read("notes.txt")?;
        let limited = session.read_with_options("notes.txt", ReadOptions::new().max_bytes(5))?;
        let grep = session.grep("omega")?;
        let glob = session.glob("*.txt")?;
        let shell = session.shell_with_options(
            "build",
            ShellOptions::new()
                .cwd("/sandbox/project")
                .env("TOKEN", "scoped"),
        )?;
        let stat = session.stat("notes.txt")?;
        Ok(json!({
            "read": read,
            "limited": limited,
            "grep": grep,
            "glob": glob,
            "shell": shell,
            "stat": stat
        }))
    }));

    let result = app.invoke("remote", "env-1", json!({})).unwrap();

    assert_eq!(result["read"], "alpha gamma");
    assert_eq!(result["limited"]["content"], "alpha");
    assert_eq!(result["limited"]["truncated"], true);
    assert_eq!(result["grep"][0]["path"], "log.txt");
    assert_eq!(result["glob"], json!(["log.txt", "notes.txt"]));
    assert_eq!(result["shell"]["stdout"], "build:/sandbox/project:scoped");
    assert_eq!(result["stat"]["size"], 11);
    let execs = captured_execs.lock().unwrap();
    assert_eq!(execs.len(), 1);
    assert_eq!(execs[0].0, "build");
    assert_eq!(
        execs[0].1.env.get("TOKEN").map(String::as_str),
        Some("scoped")
    );
}

#[test]
fn memory_session_env_provides_empty_isolated_file_helpers() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("host-secret.txt"), "do not leak").unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .agent(AgentDefinition::webhook("empty", |ctx| {
            let session = ctx.session_with_id_and_env("empty", MemorySessionEnv::new("/workspace"));
            let host_visible = session.exists("host-secret.txt")?;
            session.write("docs/notes.txt", "alpha\nbeta")?;
            session.write("docs/todo.md", "omega")?;
            let listing = session.readdir("docs")?;
            let grep = session.grep("alpha")?;
            let glob = session.glob("docs/*.txt")?;
            let stat = session.stat("docs/notes.txt")?;
            let shell_error = session.shell("pwd").unwrap_err().error_type();
            Ok(json!({
                "hostVisible": host_visible,
                "listing": listing,
                "grep": grep,
                "glob": glob,
                "stat": stat,
                "shellError": shell_error
            }))
        }));

    let result = app.invoke("empty", "mem-1", json!({})).unwrap();

    assert_eq!(result["hostVisible"], false);
    assert_eq!(result["listing"], json!(["notes.txt", "todo.md"]));
    assert_eq!(result["grep"][0]["path"], "docs/notes.txt");
    assert_eq!(result["glob"], json!(["docs/notes.txt"]));
    assert_eq!(result["stat"]["isFile"], true);
    assert_eq!(result["stat"]["size"], 10);
    assert_eq!(result["shellError"], "handler_error");
}

#[test]
fn http_session_env_adapts_remote_shell_and_file_helpers() {
    let transport = RecordingHttpSessionTransport::new();
    let env = HttpSessionEnv::new("https://sandbox.example/session", "/workspace/project")
        .header("Authorization", "Bearer test-token")
        .transport(transport.clone());
    let app = AgentApp::new().agent(AgentDefinition::webhook("remote", move |ctx| {
        let session = ctx.session_with_id_and_env("remote", env.clone());
        let shell = session.shell_with_options(
            "npm test",
            ShellOptions::new()
                .cwd("/workspace/project/app")
                .env("TOKEN", "abc")
                .timeout(Duration::from_secs(2)),
        )?;
        session.write("notes.txt", "alpha")?;
        let read = session.read("notes.txt")?;
        let stat = session.stat("notes.txt")?;
        let readdir = session.readdir(".")?;
        let exists = session.exists("notes.txt")?;
        session.mkdir("src")?;
        session.rm("notes.txt", false)?;
        Ok(json!({
            "shell": shell,
            "read": read,
            "stat": stat,
            "readdir": readdir,
            "exists": exists
        }))
    }));

    let result = app.invoke("remote", "http-env", json!({})).unwrap();

    assert_eq!(result["shell"]["stdout"], "remote:npm test");
    assert_eq!(result["shell"]["exitCode"], 0);
    assert_eq!(result["read"], "remote-file:notes.txt");
    assert_eq!(result["stat"]["isFile"], true);
    assert_eq!(result["stat"]["size"], 18);
    assert_eq!(result["stat"]["modifiedUnixMs"], 42);
    assert_eq!(result["readdir"], json!(["notes.txt", "src"]));
    assert_eq!(result["exists"], true);

    let requests = transport.requests();
    let bodies = requests
        .iter()
        .map(|(_, _, body)| body.clone())
        .collect::<Vec<_>>();
    assert!(requests
        .iter()
        .all(|(url, _, _)| url == "https://sandbox.example/session"));
    assert!(requests.iter().all(|(_, headers, _)| {
        headers.get("Authorization").map(String::as_str) == Some("Bearer test-token")
    }));
    assert_eq!(bodies.len(), 8);
    assert_eq!(bodies[0]["op"], "exec");
    assert_eq!(bodies[0]["command"], "npm test");
    assert_eq!(bodies[0]["cwd"], "/workspace/project/app");
    assert_eq!(bodies[0]["env"]["TOKEN"], "abc");
    assert_eq!(bodies[0]["timeoutMs"], 2000);
    assert_eq!(
        bodies[1],
        json!({ "op": "write", "path": "notes.txt", "content": "alpha" })
    );
    assert_eq!(
        bodies[7],
        json!({ "op": "rm", "path": "notes.txt", "recursive": false })
    );
}

#[test]
fn http_session_env_writes_binary_content_as_base64() {
    let transport = RecordingHttpSessionTransport::new();
    let env = HttpSessionEnv::new("https://sandbox.example/session", "/workspace")
        .transport(transport.clone());
    let app = AgentApp::new().agent(AgentDefinition::webhook("remote", move |ctx| {
        let session = ctx.session_with_id_and_env("remote", env.clone());
        session.write("bin/blob.dat", [0xff, 0x00, 0x01])?;
        Ok(json!({ "ok": true }))
    }));

    let result = app.invoke("remote", "http-env-binary", json!({})).unwrap();

    assert_eq!(result["ok"], true);
    let requests = transport.requests();
    assert_eq!(
        requests[0].2,
        json!({
            "op": "write",
            "path": "bin/blob.dat",
            "contentBase64": "/wAB"
        })
    );
}

#[test]
fn http_session_env_surfaces_remote_error_envelopes() {
    let transport = RecordingHttpSessionTransport::new();
    let env =
        HttpSessionEnv::new("https://sandbox.example/session", "/workspace").transport(transport);
    let app = AgentApp::new().agent(AgentDefinition::webhook("remote", move |ctx| {
        let session = ctx.session_with_id_and_env("remote", env.clone());
        let _ = session.read("missing.txt")?;
        Ok(json!({}))
    }));

    let err = app
        .invoke("remote", "http-env-error", json!({}))
        .unwrap_err()
        .to_string();

    assert!(err.contains("remote read failed"));
    assert!(err.contains("missing remote file"));
}

#[test]
fn registered_commands_are_explicit_host_capabilities() {
    let app = AgentApp::new()
        .command(CommandDef::new("gh", |args| {
            Ok(agentic_harness::ShellOutput {
                stdout: format!("gh {}", args.join(" ")),
                stderr: String::new(),
                exit_code: 0,
            })
        }))
        .agent(AgentDefinition::webhook("cmd", |ctx| {
            let output = ctx.command("gh", ["issue", "list"])?;
            let missing = ctx.command("npm", ["test"]).unwrap_err().error_type();
            Ok(json!({
                "stdout": output.stdout,
                "missing": missing
            }))
        }));

    let result = app.invoke("cmd", "cmd-1", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "stdout": "gh issue list",
            "missing": "command_not_found"
        })
    );
}

#[test]
fn custom_tools_are_registered_invokable_and_passed_to_model_requests() {
    let lookup_tool = ToolDef::new(
        "lookup",
        "Look up a customer",
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" }
            },
            "required": ["id"]
        }),
        |args| Ok(format!("customer:{}", args["id"].as_str().unwrap())),
    );
    let summarize_tool = ToolDef::new(
        "summarize",
        "Summarize text",
        json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" }
            },
            "required": ["text"]
        }),
        |args| Ok(format!("summary:{}", args["text"].as_str().unwrap())),
    );

    let app = AgentApp::new()
        .tool(lookup_tool)
        .model("local/tools", ToolInspectModel)
        .default_model("local/tools")
        .agent(AgentDefinition::webhook("tools", move |ctx| {
            let direct = ctx.tool("lookup", json!({ "id": "42" }))?;
            let mut session = ctx.session();
            let response = session.prompt_with_options(
                "inspect tools",
                PromptOptions::new().tool(summarize_tool.clone()),
            )?;
            let tools: serde_json::Value = serde_json::from_str(&response.text)?;
            Ok(json!({
                "direct": direct,
                "tools": tools
            }))
        }));

    let result = app.invoke("tools", "tools-1", json!({})).unwrap();

    assert_eq!(result["direct"], "customer:42");
    assert_eq!(result["tools"][0]["name"], "lookup");
    assert_eq!(result["tools"][1]["name"], "summarize");
    assert_eq!(result["tools"][1]["parameters"]["required"][0], "text");
}

#[test]
fn mcp_client_tools_are_exposed_as_custom_tools() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let tools = mcp_tools_from_client(
        "docs server",
        FakeMcpClient {
            calls: calls.clone(),
        },
    )
    .unwrap();

    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name(), "mcp__docs_server__search_docs");
    assert_eq!(
        tools[0].spec().parameters,
        json!({
            "type": "object",
            "properties": { "q": { "type": "string" } },
            "required": ["q"]
        })
    );

    let tool = tools[0].clone();
    let app = AgentApp::new()
        .tool(tool)
        .agent(AgentDefinition::webhook("ask", |ctx| {
            Ok(json!({
                "tool": ctx.tool("mcp__docs_server__search_docs", json!({ "q": "Ada" }))?
            }))
        }));

    assert_eq!(
        app.invoke("ask", "mcp", json!({})).unwrap(),
        json!({ "tool": "found Ada" })
    );

    assert_eq!(
        *calls.lock().unwrap(),
        vec![("search.docs".to_string(), json!({ "q": "Ada" }))]
    );
}

#[test]
fn mcp_server_options_expose_legacy_sse_transport_selection() {
    let options = agentic_harness::McpServerOptions::new("https://mcp.example/sse")
        .transport(McpTransport::Sse);

    assert_eq!(
        serde_json::to_value(options).unwrap()["transport"],
        json!("sse")
    );
}

#[test]
fn session_executes_model_requested_tool_calls_before_returning_final_response() {
    let calls = Arc::new(Mutex::new(0));
    let app = AgentApp::new()
        .tool(ToolDef::new(
            "lookup",
            "Look up a customer",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
            |args| Ok(format!("customer:{}", args["id"].as_str().unwrap())),
        ))
        .model(
            "local/tool-calling",
            ToolCallingModel {
                calls: calls.clone(),
            },
        )
        .default_model("local/tool-calling")
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let response = session.prompt_with_options("find customer", PromptOptions::new())?;
            Ok(json!({
                "text": response.text,
                "history": session.history()
            }))
        }));

    let result = app.invoke("assistant", "tools", json!({})).unwrap();

    assert_eq!(*calls.lock().unwrap(), 2);
    assert!(result["text"].as_str().unwrap().contains("customer:42"));
    assert!(result["history"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool"
            && message["content"].as_str().unwrap().contains("customer:42")));
}

#[test]
fn session_exposes_builtin_tools_to_models_and_executes_builtin_tool_calls() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("notes.txt"), "alpha beta gamma").unwrap();
    let calls = Arc::new(Mutex::new(0));
    let app = AgentApp::new()
        .with_workspace(temp.path())
        .model(
            "local/builtin-tools",
            BuiltinToolModel {
                calls: calls.clone(),
            },
        )
        .default_model("local/builtin-tools")
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let response = session.prompt_with_options("read the notes", PromptOptions::new())?;
            Ok(json!({
                "text": response.text,
                "history": session.history()
            }))
        }));

    let result = app.invoke("assistant", "builtin-tools", json!({})).unwrap();

    assert_eq!(*calls.lock().unwrap(), 2);
    assert_eq!(result["text"], "builtin final");
    assert!(result["history"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool"
            && message["content"].as_str().unwrap().contains("alpha")));
}

#[test]
fn custom_tools_reject_duplicate_and_builtin_names() {
    let duplicate = AgentApp::new()
        .tool(ToolDef::new("lookup", "one", json!({}), |_| {
            Ok("one".to_string())
        }))
        .tool(ToolDef::new("lookup", "two", json!({}), |_| {
            Ok("two".to_string())
        }))
        .agent(AgentDefinition::webhook("tools", |ctx| {
            let mut session = ctx.session();
            session
                .prompt_with_options("test", PromptOptions::new())
                .map(|_| json!({}))
        }));
    let err = duplicate.invoke("tools", "dup", json!({})).unwrap_err();
    assert_eq!(err.error_type(), "duplicate_tool");

    let builtin = AgentApp::new()
        .tool(ToolDef::new("read", "conflict", json!({}), |_| {
            Ok("bad".to_string())
        }))
        .agent(AgentDefinition::webhook("tools", |ctx| {
            let mut session = ctx.session();
            session
                .prompt_with_options("test", PromptOptions::new())
                .map(|_| json!({}))
        }));
    let err = builtin.invoke("tools", "builtin", json!({})).unwrap_err();
    assert_eq!(err.error_type(), "tool_name_conflict");
}

#[test]
fn load_workspace_context_discovers_roles_and_skills() {
    let temp = tempfile::tempdir().unwrap();
    let roles = temp.path().join(".agentic-harness/roles");
    let skills = temp.path().join(".agents/skills/greet");
    fs::create_dir_all(&roles).unwrap();
    fs::create_dir_all(&skills).unwrap();
    fs::write(
        roles.join("greeter.md"),
        "---\ndescription: Friendly greeter\nmodel: openai/gpt-5.5\n---\nUse warm language.\n",
    )
    .unwrap();
    fs::write(
        skills.join("SKILL.md"),
        "---\nname: greet\ndescription: Build a greeting\n---\nReturn a short greeting.\n",
    )
    .unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .load_workspace_context()
        .unwrap()
        .agent(AgentDefinition::webhook("inspect", |ctx| {
            Ok(json!({
                "role": ctx.role("greeter").unwrap().description,
                "roleModel": ctx.role("greeter").unwrap().model,
                "skill": ctx.skill("greet").unwrap().instructions.trim()
            }))
        }));

    let result = app.invoke("inspect", "ctx-1", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "role": "Friendly greeter",
            "roleModel": "openai/gpt-5.5",
            "skill": "Return a short greeting."
        })
    );
}

#[test]
fn provider_settings_merge_like_original_runtime_config() {
    let mut base = ProvidersConfig::new();
    base.insert(
        "openai",
        ProviderSettings::new()
            .base_url("https://gateway.example/v1")
            .header("X-Base", "base")
            .api_key("base-key"),
    );

    let mut overrides = ProvidersConfig::new();
    overrides.insert(
        "openai",
        ProviderSettings::new()
            .header("X-Override", "override")
            .api_key("override-key"),
    );
    overrides.insert(
        "anthropic",
        ProviderSettings::new().base_url("https://anthropic.example"),
    );

    let merged = base.merged(&overrides);

    assert_eq!(
        merged.get("openai").unwrap().base_url.as_deref(),
        Some("https://gateway.example/v1")
    );
    assert_eq!(
        merged.get("openai").unwrap().api_key.as_deref(),
        Some("override-key")
    );
    assert_eq!(
        merged.get("openai").unwrap().headers.get("X-Base").unwrap(),
        "base"
    );
    assert_eq!(
        merged
            .get("openai")
            .unwrap()
            .headers
            .get("X-Override")
            .unwrap(),
        "override"
    );
    assert_eq!(
        merged.get("anthropic").unwrap().base_url.as_deref(),
        Some("https://anthropic.example")
    );
}

#[test]
fn provider_settings_resolve_api_keys_from_environment_names() {
    let expected = std::env::var("PATH").unwrap();
    let settings = ProviderSettings::new().api_key_env("PATH");

    assert_eq!(settings.api_key_env.as_deref(), Some("PATH"));
    assert_eq!(
        settings.resolved_api_key().as_deref(),
        Some(expected.as_str())
    );
    assert_eq!(
        settings
            .clone()
            .api_key("literal-key")
            .resolved_api_key()
            .as_deref(),
        Some("literal-key")
    );
}

#[test]
fn runtime_config_file_registers_provider_defaults_and_openai_models() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("agentic-harness.json");
    fs::write(
        &config_path,
        r#"{
          "defaultModel": "openai/gpt-test",
          "openaiCompatibleModels": ["openai/gpt-test"],
          "providers": {
            "openai": {
              "baseUrl": "https://gateway.example/v1",
              "apiKeyEnv": "OPENAI_API_KEY",
              "headers": {
                "X-Gateway": "yes"
              }
            }
          }
        }"#,
    )
    .unwrap();

    let parsed = AgentRuntimeConfig::from_path(&config_path).unwrap();
    assert_eq!(parsed.default_model.as_deref(), Some("openai/gpt-test"));

    let app = AgentApp::new()
        .load_runtime_config(&config_path)
        .unwrap()
        .agent(AgentDefinition::webhook("inspect", |ctx| {
            let settings = ctx.provider_settings("openai").unwrap();
            Ok(json!({
                "baseUrl": settings.base_url,
                "apiKeyEnv": settings.api_key_env,
                "header": settings.headers.get("X-Gateway")
            }))
        }));

    assert_eq!(app.default_model_name(), Some("openai/gpt-test"));
    assert_eq!(app.model_names(), vec!["openai/gpt-test"]);
    assert_eq!(
        app.invoke("inspect", "runtime-config", json!({})).unwrap(),
        json!({
            "baseUrl": "https://gateway.example/v1",
            "apiKeyEnv": "OPENAI_API_KEY",
            "header": "yes"
        })
    );
}

#[test]
fn runtime_config_rejects_invalid_model_strings() {
    let temp = tempfile::tempdir().unwrap();
    let config_path = temp.path().join("agentic-harness.json");
    fs::write(
        &config_path,
        r#"{ "defaultModel": "not-a-provider-model" }"#,
    )
    .unwrap();

    let err = match AgentApp::new().load_runtime_config(&config_path) {
        Ok(_) => panic!("invalid runtime config unexpectedly loaded"),
        Err(err) => err,
    };

    assert_eq!(err.error_type(), "invalid_model");
}

#[test]
fn load_workspace_context_discovers_root_runtime_config_file() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(
        temp.path().join("agentic-harness.json"),
        r#"{
          "defaultModel": "openai/gpt-root",
          "openaiCompatibleModels": ["openai/gpt-root"],
          "providers": {
            "openai": {
              "baseUrl": "https://root-gateway.example/v1",
              "apiKeyEnv": "OPENAI_API_KEY"
            }
          }
        }"#,
    )
    .unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .load_workspace_context()
        .unwrap();

    assert_eq!(app.default_model_name(), Some("openai/gpt-root"));
    assert_eq!(app.model_names(), vec!["openai/gpt-root"]);
}

#[test]
fn load_workspace_context_discovers_dot_agentic_harness_runtime_config_file() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join(".agentic-harness")).unwrap();
    fs::write(
        temp.path().join(".agentic-harness/config.json"),
        r#"{
          "defaultModel": "openai/gpt-dot",
          "openaiCompatibleModels": ["openai/gpt-dot"]
        }"#,
    )
    .unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .load_workspace_context()
        .unwrap();

    assert_eq!(app.default_model_name(), Some("openai/gpt-dot"));
    assert_eq!(app.model_names(), vec!["openai/gpt-dot"]);
}

#[test]
fn configured_models_resolve_default_role_and_call_precedence() {
    let temp = tempfile::tempdir().unwrap();
    let roles = temp.path().join(".agentic-harness/roles");
    fs::create_dir_all(&roles).unwrap();
    fs::write(
        roles.join("greeter.md"),
        "---\ndescription: Friendly greeter\nmodel: local/role\n---\nUse warm language.\n",
    )
    .unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .load_workspace_context()
        .unwrap()
        .model("local/default", NamedModel("default"))
        .model("local/role", NamedModel("role"))
        .model("local/call", NamedModel("call"))
        .default_model("local/default")
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let default = session.prompt_with_options("default", PromptOptions::new())?;
            let role = session.prompt_with_options("role", PromptOptions::new().role("greeter"))?;
            let call = session.prompt_with_options(
                "call",
                PromptOptions::new().role("greeter").model("local/call"),
            )?;

            Ok(json!({
                "default": default.text,
                "role": role.text,
                "call": call.text
            }))
        }));

    let result = app.invoke("assistant", "models", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "default": "default:false",
            "role": "role:true",
            "call": "call:true"
        })
    );
}

#[test]
fn openai_compatible_model_builds_chat_request_and_reads_response() {
    let model = OpenAiCompatibleModel::new(
        "gpt-test",
        ProviderSettings::new()
            .base_url("https://gateway.example/v1")
            .api_key("test-key")
            .header("X-Gateway", "yes"),
    );
    let request = model.request_json(&ModelRequest {
        system: "system prompt".to_string(),
        messages: vec![ModelMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
        }],
        tools: Vec::new(),
    });
    let response = model
        .parse_response(
            "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"provider ok\"}}]}",
        )
        .unwrap();
    let tool_response = model
        .parse_response(
            "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":null,\"tool_calls\":[{\"id\":\"call-1\",\"type\":\"function\",\"function\":{\"name\":\"lookup\",\"arguments\":\"{\\\"id\\\":\\\"42\\\"}\"}}]}}]}",
        )
        .unwrap();

    assert_eq!(response.text, "provider ok");
    assert_eq!(
        tool_response.tool_calls,
        vec![ToolCall {
            id: "call-1".to_string(),
            name: "lookup".to_string(),
            arguments: json!({ "id": "42" })
        }]
    );
    assert_eq!(request["model"], "gpt-test");
    assert_eq!(request["messages"][0]["role"], "system");
    assert_eq!(request["messages"][0]["content"], "system prompt");
    assert_eq!(request["messages"][1]["role"], "user");
    assert_eq!(request["messages"][1]["content"], "hello");
    assert_eq!(
        model.endpoint(),
        "https://gateway.example/v1/chat/completions"
    );
}

#[test]
fn http_dispatch_preserves_agent_route_shape_and_error_envelope() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
        Ok(json!({ "id": ctx.id(), "payload": ctx.payload_value() }))
    }));

    let ok = app.handle_http("POST", "/agents/hello/h-1", br#"{"x":1}"#);
    assert_eq!(ok.status, 200);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&ok.body).unwrap(),
        json!({ "result": { "id": "h-1", "payload": { "x": 1 } } })
    );

    let missing = app.handle_http("POST", "/agents/missing/h-1", b"{}");
    assert_eq!(missing.status, 404);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&missing.body).unwrap(),
        json!({
            "error": {
                "type": "agent_not_found",
                "message": "Agent \"missing\" was not found.",
                "details": "Available agents: hello"
            }
        })
    );
}

#[test]
fn http_dispatch_can_return_sse_event_stream() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
        Ok(json!({ "id": ctx.id(), "payload": ctx.payload_value() }))
    }));

    let response = app.handle_http("POST", "/agents/hello/h-1/events", br#"{"x":1}"#);
    let body = String::from_utf8(response.body).unwrap();

    assert_eq!(response.status, 200);
    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name == "content-type" && value == "text/event-stream"));
    assert!(body.contains("event: agent_start\n"));
    assert!(body.contains("event: result\n"));
    assert!(body.contains("\"payload\":{\"x\":1}"));
    assert!(body.contains("event: idle\n"));
}

#[test]
fn http_request_accept_header_can_select_sse_on_agent_route() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
        Ok(json!({ "id": ctx.id(), "payload": ctx.payload_value() }))
    }));

    let response = app.handle_request(
        HttpRequest::new("POST", "/agents/hello/h-accept", br#"{"x":1}"#)
            .header("Accept", "application/json, text/event-stream"),
    );
    let body = String::from_utf8(response.body).unwrap();

    assert_eq!(response.status, 200);
    assert!(response
        .headers
        .iter()
        .any(|(name, value)| name == "content-type" && value == "text/event-stream"));
    assert!(body.contains("event: agent_start\n"));
    assert!(body.contains("event: result\n"));
    assert!(body.contains("\"id\":\"h-accept\""));
    assert!(body.contains("event: idle\n"));
}

#[test]
fn runtime_event_encodes_sse_frames() {
    let frame = RuntimeEvent::new("text_delta", json!({ "text": "hello" }))
        .to_sse_frame()
        .unwrap();

    assert_eq!(frame, "event: text_delta\ndata: {\"text\":\"hello\"}\n\n");
}

#[test]
fn http_event_stream_includes_model_tool_events() {
    let calls = Arc::new(Mutex::new(0));
    let app = AgentApp::new()
        .tool(ToolDef::new(
            "lookup",
            "Look up a customer",
            json!({ "type": "object", "properties": { "id": { "type": "string" } } }),
            |args| Ok(format!("customer:{}", args["id"].as_str().unwrap())),
        ))
        .model(
            "local/tool-calling",
            ToolCallingModel {
                calls: calls.clone(),
            },
        )
        .default_model("local/tool-calling")
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let response = session.prompt_with_options("find customer", PromptOptions::new())?;
            Ok(json!({ "text": response.text }))
        }));

    let response = app.handle_http("POST", "/agents/assistant/sse-tools/events", b"{}");
    let body = String::from_utf8(response.body).unwrap();

    assert_eq!(response.status, 200);
    assert!(body.contains("event: agent_start\n"));
    assert!(body.contains("event: tool_start\n"));
    assert!(body.contains("\"toolName\":\"lookup\""));
    assert!(body.contains("event: tool_end\n"));
    assert!(body.contains("\"isError\":false"));
    assert!(body.contains("event: result\n"));
    assert!(body.contains("event: idle\n"));
}

#[test]
fn runtime_events_can_stream_to_a_live_sink() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("assistant", |ctx| {
        let mut session = ctx.session();
        let response = session.prompt("hello", &TestModel)?;
        Ok(json!({ "text": response.text }))
    }));
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();

    let result = app
        .invoke_with_event_sink("assistant", "live-1", json!({}), move |event| {
            seen.lock()
                .unwrap()
                .push((event.event().to_string(), event.data().clone()));
        })
        .unwrap();
    let events = events.lock().unwrap();
    let names = events
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();

    assert_eq!(result["text"], "model saw: hello");
    assert_eq!(
        names,
        vec!["agent_start", "text_delta", "turn_end", "result", "idle"]
    );
    assert_eq!(events[1].1, json!({ "text": "model saw: hello" }));
    assert_eq!(events[3].1["text"], "model saw: hello");
}

#[test]
fn cli_only_agents_are_not_public_webhooks_but_can_be_invoked_directly() {
    let app = AgentApp::new().agent(AgentDefinition::cli_only("triage", |_| {
        Ok(json!({ "status": "ok" }))
    }));

    assert_eq!(
        app.invoke("triage", "ci-1", json!({})).unwrap(),
        json!({ "status": "ok" })
    );

    let public = app.handle_http("POST", "/agents/triage/ci-1", b"{}");
    assert_eq!(public.status, 404);
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&public.body).unwrap()["error"]["type"],
        "route_not_found"
    );
}

#[test]
fn invalid_payload_returns_typed_error() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("hello", |ctx| {
        let _payload: NamePayload = ctx.payload()?;
        Ok(json!({ "ok": true }))
    }));

    let err = app
        .invoke("hello", "bad", json!({ "name": 42 }))
        .unwrap_err();

    assert!(matches!(err, AgenticHarnessError::InvalidPayload { .. }));
    assert_eq!(err.error_type(), "invalid_payload");
}

struct TestModel;

impl ModelClient for TestModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let last = request.messages.last().unwrap();
        Ok(PromptResponse::text(format!("model saw: {}", last.content)))
    }
}

struct StructuredModel;

impl ModelClient for StructuredModel {
    fn complete(&self, _request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        Ok(PromptResponse::text("First pass\n---RESULT_START---\n{\"answer\":\"old\",\"confidence\":1}\n---RESULT_END---\nFinal\n---RESULT_START---\n{\"answer\":\"native\",\"confidence\":9}\n---RESULT_END---"))
    }
}

struct SchemaInstructionModel;

impl ModelClient for SchemaInstructionModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let prompt = request.messages.last().unwrap().content.as_str();
        assert!(prompt.contains("When complete, you MUST output your result"));
        assert!(prompt.contains("---RESULT_START---"));
        assert!(prompt.contains("---RESULT_END---"));
        assert!(prompt.contains("\"answer\""));
        assert!(prompt.contains("\"confidence\""));
        Ok(PromptResponse::text(
            "---RESULT_START---\n{\"answer\":\"schema-guided\",\"confidence\":8}\n---RESULT_END---",
        ))
    }
}

struct CountingModel;

impl ModelClient for CountingModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        Ok(PromptResponse::text(format!(
            "{} messages",
            request.messages.len()
        )))
    }
}

struct SystemPromptModel;

impl ModelClient for SystemPromptModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        Ok(PromptResponse::text(request.system))
    }
}

struct ContextCaptureModel;

impl ModelClient for ContextCaptureModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        Ok(PromptResponse::text(
            request
                .messages
                .iter()
                .map(|message| format!("{}:{}", message.role, message.content))
                .collect::<Vec<_>>()
                .join("\n---\n"),
        ))
    }
}

#[derive(Clone)]
struct AutoCompactionModel {
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}

impl ModelClient for AutoCompactionModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        self.requests.lock().unwrap().push(request.clone());
        if request
            .system
            .contains("Summarize this Agentic Harness session")
        {
            let transcript = request
                .messages
                .first()
                .map(|message| message.content.as_str())
                .unwrap_or("");
            assert!(transcript.contains("old-context-alpha"));
            assert!(transcript.contains("old-context-beta"));
            assert!(!transcript.contains("current question"));
            return Ok(PromptResponse::text("summary: earlier work"));
        }

        let last = request
            .messages
            .last()
            .map(|message| message.content.as_str())
            .unwrap_or("");
        Ok(PromptResponse::text(format!("answer to {last}")))
    }
}

#[test]
fn prompt_response_extracts_last_structured_json_result() {
    let response = StructuredModel
        .complete(ModelRequest {
            system: String::new(),
            messages: Vec::new(),
            tools: Vec::new(),
        })
        .unwrap();

    let result: StructuredResult = response.result_json().unwrap();

    assert_eq!(
        result,
        StructuredResult {
            answer: "native".to_string(),
            confidence: 9
        }
    );
}

#[test]
fn prompt_options_can_add_schema_guidance_and_return_typed_json() {
    let app = AgentApp::new()
        .model("local/schema", SchemaInstructionModel)
        .default_model("local/schema")
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let result: StructuredResult = session.prompt_json_with_options(
                "Find the best answer.",
                PromptOptions::new().result_schema(json!({
                    "type": "object",
                    "properties": {
                        "answer": { "type": "string" },
                        "confidence": { "type": "integer" }
                    },
                    "required": ["answer", "confidence"]
                })),
            )?;
            Ok(serde_json::to_value(result)?)
        }));

    let result = app.invoke("assistant", "schema", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "answer": "schema-guided",
            "confidence": 8
        })
    );
}

#[test]
fn session_prompt_records_history_without_baking_in_a_provider() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("assistant", |ctx| {
        let mut session = ctx.session();
        let first = session.prompt("say hello", &TestModel)?;
        let second = session.prompt("say goodbye", &TestModel)?;

        Ok(json!({
            "first": first.text,
            "second": second.text,
            "history": session.history().iter().map(|m| m.role.as_str()).collect::<Vec<_>>()
        }))
    }));

    let result = app.invoke("assistant", "s1", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "first": "model saw: say hello",
            "second": "model saw: say goodbye",
            "history": ["user", "assistant", "user", "assistant"]
        })
    );
}

#[test]
fn session_task_runs_in_detached_child_history() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("assistant", |ctx| {
        let mut session = ctx.session();
        let parent = session.prompt("parent", &CountingModel)?;
        let child_first = session.task_with_id("research", "child one", &CountingModel)?;
        let child_second = session.task_with_id("research", "child two", &CountingModel)?;

        Ok(json!({
            "parent": parent.text,
            "childFirst": child_first.text,
            "childSecond": child_second.text,
            "parentHistory": session.history().len()
        }))
    }));

    let result = app.invoke("assistant", "task-1", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "parent": "1 messages",
            "childFirst": "1 messages",
            "childSecond": "3 messages",
            "parentHistory": 2
        })
    );
}

#[test]
fn session_history_persists_across_invocations_with_same_request_id() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("memory", |ctx| {
        let mut session = ctx.session();
        let response = session.prompt(ctx.payload_value()["text"].as_str().unwrap(), &TestModel)?;
        Ok(json!({
            "text": response.text,
            "historyLength": session.history().len()
        }))
    }));

    let first = app
        .invoke("memory", "same-id", json!({ "text": "first" }))
        .unwrap();
    let second = app
        .invoke("memory", "same-id", json!({ "text": "second" }))
        .unwrap();
    let isolated = app
        .invoke("memory", "other-id", json!({ "text": "isolated" }))
        .unwrap();

    assert_eq!(first["historyLength"], 2);
    assert_eq!(second["historyLength"], 4);
    assert_eq!(isolated["historyLength"], 2);
}

#[test]
fn file_session_store_persists_history_across_app_instances() {
    fn app(store: &std::path::Path) -> AgentApp {
        AgentApp::new()
            .file_session_store(store)
            .agent(AgentDefinition::webhook("memory", |ctx| {
                let mut session = ctx.session();
                session.prompt(ctx.payload_value()["text"].as_str().unwrap(), &TestModel)?;
                Ok(json!({ "historyLength": session.history().len() }))
            }))
    }

    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("sessions");

    let first = app(&store)
        .invoke("memory", "same-id", json!({ "text": "first" }))
        .unwrap();
    let second = app(&store)
        .invoke("memory", "same-id", json!({ "text": "second" }))
        .unwrap();

    assert_eq!(first["historyLength"], 2);
    assert_eq!(second["historyLength"], 4);
}

#[test]
fn file_session_store_persists_compaction_and_branch_summary_entries() {
    let temp = tempfile::tempdir().unwrap();
    let store = temp.path().join("sessions");
    let app = AgentApp::new()
        .file_session_store(&store)
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            session.prompt("first message", &TestModel)?;
            session.prompt("second message", &TestModel)?;
            session.append_branch_summary(
                "child task found the docs",
                "task-1",
                json!({ "task": "docs" }),
            )?;
            session.compact_with_summary(
                "Earlier discussion summarized.",
                CompactionOptions::new()
                    .keep_recent_messages(2)
                    .tokens_before(123),
            )?;
            Ok(json!({ "history": session.history() }))
        }));

    let compacted = app.invoke("assistant", "compact", json!({})).unwrap();
    assert!(compacted["history"][0]["content"]
        .as_str()
        .unwrap()
        .starts_with("[Context Summary]\n\nEarlier discussion summarized."));
    assert!(compacted["history"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["content"]
            .as_str()
            .unwrap()
            .contains("[Branch Summary]\n\nchild task found the docs")));

    let files = fs::read_dir(&store)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(files.len(), 1);
    let stored: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(files[0].path()).unwrap()).unwrap();
    assert_eq!(stored["version"], 2);
    assert!(stored["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["type"] == "branch_summary"));
    assert!(stored["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| { entry["type"] == "compaction" && entry["tokensBefore"] == 123 }));

    let resumed = AgentApp::new()
        .file_session_store(&store)
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let response = session.prompt("third message", &ContextCaptureModel)?;
            Ok(json!({ "context": response.text }))
        }));
    let result = resumed.invoke("assistant", "compact", json!({})).unwrap();
    let context = result["context"].as_str().unwrap();

    assert!(context.contains("[Context Summary]\n\nEarlier discussion summarized."));
    assert!(context.contains("[Branch Summary]\n\nchild task found the docs"));
    assert!(context.contains("third message"));
    assert!(!context.contains("first message"));
}

#[test]
fn prompt_options_auto_compact_over_budget_context_before_model_call() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let app = AgentApp::new()
        .model(
            "local/auto-compact",
            AutoCompactionModel {
                requests: requests.clone(),
            },
        )
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            session.prompt_with_options(
                format!("old-context-alpha {}", "a".repeat(500)),
                PromptOptions::new().model("local/auto-compact"),
            )?;
            session.prompt_with_options(
                format!("old-context-beta {}", "b".repeat(500)),
                PromptOptions::new().model("local/auto-compact"),
            )?;
            let response = session.prompt_with_options(
                "current question",
                PromptOptions::new().model("local/auto-compact").compaction(
                    CompactionSettings::new()
                        .context_window_tokens(180)
                        .reserve_tokens(20)
                        .keep_recent_messages(1),
                ),
            )?;
            Ok(json!({
                "text": response.text,
                "history": session.history(),
                "entries": session.session_data().entries
            }))
        }));

    let result = app.invoke("assistant", "auto-compact", json!({})).unwrap();
    let captured = requests.lock().unwrap().clone();

    assert_eq!(captured.len(), 4);
    assert!(captured[2]
        .system
        .contains("Summarize this Agentic Harness session"));
    let final_messages = &captured[3].messages;
    assert!(final_messages[0]
        .content
        .starts_with("[Context Summary]\n\nsummary: earlier work"));
    assert!(final_messages
        .iter()
        .any(|message| message.content == "current question"));
    assert!(!final_messages
        .iter()
        .any(|message| message.content.contains("old-context-alpha")));
    assert_eq!(result["text"], "answer to current question");
    assert!(result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .any(
            |entry| entry["type"] == "compaction" && entry["tokensBefore"].as_u64().unwrap() > 180
        ));
}

#[test]
fn named_sessions_can_be_loaded_and_deleted() {
    let app = AgentApp::new().agent(AgentDefinition::webhook("memory", |ctx| {
        if ctx.payload_value()["delete"].as_bool().unwrap_or(false) {
            ctx.delete_session("review")?;
            return Ok(json!({ "deleted": true }));
        }
        let mut session = ctx.session_with_id("review");
        session.prompt(ctx.payload_value()["text"].as_str().unwrap(), &TestModel)?;
        Ok(json!({ "historyLength": session.history().len() }))
    }));

    assert_eq!(
        app.invoke("memory", "request-1", json!({ "text": "first" }))
            .unwrap()["historyLength"],
        2
    );
    assert_eq!(
        app.invoke("memory", "request-1", json!({ "text": "second" }))
            .unwrap()["historyLength"],
        4
    );
    app.invoke("memory", "request-1", json!({ "delete": true }))
        .unwrap();
    assert_eq!(
        app.invoke("memory", "request-1", json!({ "text": "after delete" }))
            .unwrap()["historyLength"],
        2
    );
}

#[test]
fn session_skill_turns_workspace_skill_into_a_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join(".agents/skills/greet");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: greet\ndescription: Greeting skill\n---\nUse the supplied name.\n",
    )
    .unwrap();

    let app = AgentApp::new()
        .with_workspace(temp.path())
        .load_workspace_context()
        .unwrap()
        .agent(AgentDefinition::webhook("assistant", |ctx| {
            let mut session = ctx.session();
            let response = session.skill("greet", json!({ "name": "Ada" }), &TestModel)?;
            Ok(json!({ "text": response.text }))
        }));

    let result = app.invoke("assistant", "s1", json!({})).unwrap();

    assert_eq!(
        result,
        json!({
            "text": "model saw: Use the supplied name.\n\nArguments:\n{\n  \"name\": \"Ada\"\n}"
        })
    );
}
