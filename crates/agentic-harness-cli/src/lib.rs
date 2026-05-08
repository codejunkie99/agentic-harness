use agentic_harness::{
    AgenticHarnessError, FileStat, HttpSessionEnv, SessionEnv, ShellOptions, ShellOutput,
};
use clap::{Parser, Subcommand};
use std::collections::{hash_map::DefaultHasher, BTreeMap};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

const DEV_WATCH_INTERVAL: Duration = Duration::from_millis(500);
const LLM_STATUS_PROBE_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BuildTarget {
    Native,
    NodeCompat,
    Cloudflare,
}

impl BuildTarget {
    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match value {
            "native" => Ok(Self::Native),
            "node" => Ok(Self::NodeCompat),
            "cloudflare" => Ok(Self::Cloudflare),
            other => Err(format!("Unsupported target \"{other}\". Agentic Harness supports --target native, --target node compatibility, and --target cloudflare build artifacts.").into()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::NodeCompat => "node compatibility",
            Self::Cloudflare => "cloudflare worker boundary",
        }
    }

    fn ensure_native_invocation(self, command: &str) -> Result<(), Box<dyn std::error::Error>> {
        match self {
            Self::Native | Self::NodeCompat => Ok(()),
            Self::Cloudflare => Err(format!(
                "Cloudflare Workers target is not available for `agentic-harness {command}` in the native Rust runtime. Build a native Rust server artifact with --target native or --target node compatibility instead. Workers support requires a separate non-proxy Worker-compatible runtime."
            )
            .into()),
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "agentic-harness")]
#[command(version)]
#[command(about = "Agentic Harness CLI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Build a native Agentic Harness app or Worker boundary artifacts into ./dist.
    Build {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Where dist/ is written. Default: current directory.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Build target. `node` is a native compatibility alias; `cloudflare` emits Worker boundary artifacts.
        #[arg(long, default_value = "native")]
        target: String,
        /// Worker-compatible app adapter copied to dist/agentic_harness_app.js for Cloudflare builds.
        #[arg(long = "worker-app")]
        worker_app: Option<PathBuf>,
        /// Worker-compatible WASM module copied to dist/agentic_harness_app.wasm for Cloudflare builds.
        #[arg(long = "worker-wasm")]
        worker_wasm: Option<PathBuf>,
        /// Rust crate compiled to a Worker-compatible WASM module for Cloudflare builds.
        #[arg(long = "worker-wasm-crate")]
        worker_wasm_crate: Option<PathBuf>,
        /// Load env vars from a .env-format file.
        #[arg(long = "env")]
        env_files: Vec<PathBuf>,
    },
    /// Start a native Agentic Harness development server.
    Dev {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Build target. Agentic Harness supports `native`; `node` is a compatibility alias.
        #[arg(long, default_value = "native")]
        target: String,
        /// Port for the development server.
        #[arg(long, default_value_t = 3583)]
        port: u16,
        /// Load env vars from a .env-format file.
        #[arg(long = "env")]
        env_files: Vec<PathBuf>,
    },
    /// Scaffold a native Agentic Harness agent project.
    New {
        /// Directory to create.
        path: PathBuf,
        /// Cargo package name. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
        /// Starter template: hello, triage, data, coding, code-review, test-fixer, repo-analyst, or support.
        #[arg(long, default_value = "hello")]
        template: String,
    },
    /// Start the default coding-agent flow.
    #[command(alias = "start")]
    Code {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Prompt passed to the coding agent.
        #[arg(long)]
        prompt: Option<String>,
        /// Request/session id passed to the coding agent.
        #[arg(long, default_value = "code")]
        id: String,
        /// Shell check to run after the coding agent. Repeat for multiple checks.
        #[arg(long = "test", value_name = "COMMAND")]
        test_commands: Vec<String>,
        /// Apply a unified diff patch before running checks. Repeat for multiple patches.
        #[arg(long = "apply", value_name = "PATCH")]
        apply_patches: Vec<PathBuf>,
        /// Launch a coding LLM CLI before checks: claude-code, codex, cursor, wind-server, or auto.
        #[arg(long)]
        llm: Option<String>,
        /// Skip automatically detected checks such as cargo test.
        #[arg(long)]
        no_tests: bool,
        /// Commit successful changes with this git commit message.
        #[arg(long, value_name = "MESSAGE")]
        commit: Option<String>,
        /// Create a GitHub pull request with `gh pr create --fill` after a successful run.
        #[arg(long)]
        pr: bool,
        /// Write a Markdown run summary to this path.
        #[arg(long)]
        summary: Option<PathBuf>,
        /// Write a machine-readable JSON run summary to this path.
        #[arg(long = "summary-json")]
        summary_json: Option<PathBuf>,
    },
    /// Inspect the latest coding-agent run summary.
    #[command(alias = "inspect", alias = "last")]
    Result {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print the latest run summary JSON.
        #[arg(long)]
        json: bool,
    },
    /// Manage reusable local template packs.
    #[command(alias = "templates", alias = "tpl")]
    Template {
        #[command(subcommand)]
        command: TemplateCommands,
    },
    /// Set up local authoring and execution prerequisites.
    Setup {
        #[command(subcommand)]
        command: SetupCommands,
    },
    /// Inspect or start local HTTP hosting for a native agent workspace.
    Hosting {
        #[command(subcommand)]
        command: HostingCommands,
    },
    /// Start local HTTP hosting for a native agent workspace.
    Host {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Address for the local HTTP server. Defaults to .agentic-harness/hosting.toml.
        #[arg(long)]
        addr: Option<String>,
        /// Run in watch/reload development mode instead of one-shot serve mode.
        #[arg(long)]
        dev: bool,
        /// Load env vars from a .env-format file.
        #[arg(long = "env")]
        env_files: Vec<PathBuf>,
    },
    /// Run local sandbox file and shell operations.
    Sandbox {
        #[command(subcommand)]
        command: SandboxCommands,
    },
    /// Check whether a workspace is ready to run as an Agentic Harness app.
    #[command(alias = "check")]
    Doctor {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print plain output without ANSI color.
        #[arg(long)]
        plain: bool,
        /// Print machine-readable doctor output JSON.
        #[arg(long)]
        json: bool,
    },
    /// Show a workspace dashboard with setup, template, sandbox, and next-step status.
    #[command(alias = "status")]
    Dashboard {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print plain output without ANSI color.
        #[arg(long)]
        plain: bool,
        /// Print machine-readable JSON for coding agents.
        #[arg(long)]
        json: bool,
    },
    /// Print the shortest start-here path from setup to coding.
    #[command(alias = "quickstart", alias = "onboard")]
    Guide {
        /// Workspace the guide commands should target.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// LLM environment: claude-code, codex, cursor, wind-server, or auto.
        #[arg(long, default_value = "codex")]
        env: String,
        /// Template name to use in the authoring handoff.
        #[arg(long, default_value = "coding-agent")]
        template: String,
        /// Template-authoring prompt.
        #[arg(
            long,
            default_value = "Create a reusable coding agent template for this repository"
        )]
        prompt: String,
        /// Print machine-readable guide JSON.
        #[arg(long)]
        json: bool,
    },
    /// Run a post-install smoke check for the CLI, wizard, doctor, and sandbox paths.
    Smoke {
        /// Workspace to validate with doctor and sandbox smoke checks.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable smoke output JSON.
        #[arg(long)]
        json: bool,
    },
    /// Check release packaging artifacts before publishing.
    ReleaseCheck {
        /// Repository root containing scripts, Formula, changelog, and docs.
        #[arg(long, default_value = ".")]
        root: PathBuf,
        /// Print machine-readable release-check output JSON.
        #[arg(long)]
        json: bool,
    },
    /// Stage the current CLI binary with manifest and checksums for distribution.
    Package {
        /// Directory receiving the versioned binary package folder.
        #[arg(long, default_value = "dist/packages")]
        output: PathBuf,
        /// Binary to package. Defaults to the currently running agentic-harness executable.
        #[arg(long)]
        binary: Option<PathBuf>,
        /// Print machine-readable package JSON.
        #[arg(long)]
        json: bool,
    },
    /// Print the manifest from a native Agentic Harness workspace.
    Manifest {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print Cloudflare Worker routing metadata instead of the generic agent manifest.
        #[arg(long)]
        cloudflare: bool,
    },
    /// Invoke one native Agentic Harness agent once.
    Run {
        /// Agent name.
        agent: String,
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Build target. Agentic Harness supports `native`; `node` is a compatibility alias.
        #[arg(long, default_value = "native")]
        target: String,
        /// Accepted for CLI parity; native Rust run invokes the workspace directly.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Request/session id passed to the agent.
        #[arg(long)]
        id: String,
        /// JSON payload passed to the agent.
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Load env vars from a .env-format file.
        #[arg(long = "env")]
        env_files: Vec<PathBuf>,
    },
    /// Serve a native Agentic Harness app over HTTP.
    Serve {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Address passed to the embedded native server.
        #[arg(long, default_value = "127.0.0.1:3583")]
        addr: String,
        /// Load env vars from a .env-format file.
        #[arg(long = "env")]
        env_files: Vec<PathBuf>,
    },
    /// Print native connector instructions.
    Add {
        /// Connector name, or URL/path when used with --category.
        name: Option<String>,
        /// Connector category for from-scratch instructions.
        #[arg(long)]
        category: Option<String>,
        /// Print raw connector markdown to stdout.
        #[arg(long)]
        print: bool,
    },
    /// Open the setup wizard or print the wizard dashboard.
    #[command(alias = "tui", alias = "ui")]
    Wizard {
        /// Workspace used for dashboard panels and current-workspace actions.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print the wizard dashboard without interactive prompts.
        #[arg(long)]
        plain: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TemplateCommands {
    /// Create a reusable template pack skeleton.
    #[command(alias = "create", alias = "new")]
    Init {
        /// Template pack directory to create.
        path: PathBuf,
        /// Template name. Defaults to the directory name.
        #[arg(long)]
        name: Option<String>,
        /// Agent name. Defaults to the template name.
        #[arg(long)]
        agent: Option<String>,
        /// Template description.
        #[arg(long, default_value = "Reusable Agentic Harness template")]
        description: String,
        /// Template pack version.
        #[arg(long, default_value = "0.1.0")]
        version: String,
    },
    /// List built-in and installed templates.
    #[command(alias = "ls")]
    List {
        /// Workspace whose installed templates should be listed.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Show agent, version, and description metadata.
        #[arg(long)]
        verbose: bool,
        /// Print machine-readable template registry JSON.
        #[arg(long)]
        json: bool,
    },
    /// Search built-in, workspace, user, and team templates.
    #[command(alias = "find")]
    Search {
        /// Text to match against template name, agent, version, source, or description.
        query: String,
        /// Workspace whose installed templates should be searched.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable search results JSON.
        #[arg(long)]
        json: bool,
    },
    /// Create an LLM-ready brief for authoring a reusable template pack.
    #[command(alias = "brief")]
    Author {
        /// Template name the LLM should create.
        name: String,
        /// Workspace containing the installed LLM authoring context.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Target LLM environment: claude-code, codex, cursor, wind-server, or auto.
        #[arg(long, default_value = "auto")]
        env: String,
        /// What the new agent template should do.
        #[arg(long)]
        prompt: Option<String>,
        /// Launch the selected LLM command with the generated brief path.
        #[arg(long)]
        open: bool,
        /// Print machine-readable authoring brief metadata.
        #[arg(long)]
        json: bool,
    },
    /// Preview a built-in, installed, or local template pack.
    #[command(alias = "preview")]
    Show {
        /// Template name or template pack directory.
        template: String,
        /// Workspace whose installed templates should be searched.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable template preview JSON.
        #[arg(long)]
        json: bool,
    },
    /// Validate a local template pack.
    #[command(alias = "check")]
    Validate {
        /// Template pack directory.
        path: PathBuf,
    },
    /// Export a built-in, installed, or local template pack to a directory.
    #[command(alias = "pack")]
    Export {
        /// Template name or template pack directory.
        template: String,
        /// Workspace whose installed templates should be searched.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Output directory receiving the exported template pack.
        #[arg(long)]
        output: PathBuf,
    },
    /// Install a local template pack into the workspace.
    #[command(alias = "add")]
    Install {
        /// Template pack directory.
        path: PathBuf,
        /// Workspace receiving the installed template.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Registry scope: workspace, user, or team.
        #[arg(long, default_value = "workspace")]
        scope: String,
        /// Override the installed template name.
        #[arg(long)]
        name: Option<String>,
    },
    /// Import a template pack into the workspace registry.
    #[command(alias = "use")]
    Import {
        /// Template pack directory.
        path: PathBuf,
        /// Workspace receiving the imported template.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Registry scope: workspace, user, or team.
        #[arg(long, default_value = "workspace")]
        scope: String,
        /// Override the imported template name.
        #[arg(long)]
        name: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
enum SetupCommands {
    /// Install or print LLM authoring instructions.
    Llm {
        /// Workspace receiving the LLM authoring setup.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Target environment: claude-code, codex, cursor, wind-server, or auto.
        #[arg(long, default_value = "auto")]
        env: String,
        /// Print instructions without writing files.
        #[arg(long)]
        print: bool,
    },
    /// Configure local or remote sandbox execution.
    Sandbox {
        /// Workspace receiving sandbox setup.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Sandbox target: local, vercel, daytona, e2b, or custom.
        #[arg(long, default_value = "local")]
        target: String,
        /// HTTP SessionEnv endpoint for remote sandbox operations.
        #[arg(long)]
        endpoint: Option<String>,
        /// Print connector instructions without writing files.
        #[arg(long)]
        print: bool,
    },
    /// Configure local HTTP hosting for a native agent workspace.
    Hosting {
        /// Workspace receiving hosting setup.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Address for the local HTTP server.
        #[arg(long, default_value = "127.0.0.1:3583")]
        addr: String,
    },
}

#[derive(Debug, Subcommand)]
enum HostingCommands {
    /// Show local hosting config, URLs, and server capabilities.
    Status {
        /// Workspace containing .agentic-harness/hosting.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable local hosting status JSON.
        #[arg(long)]
        json: bool,
    },
    /// Start local HTTP hosting for a native agent workspace.
    Start {
        /// Cargo project containing the native Agentic Harness app.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Address for the local HTTP server. Defaults to .agentic-harness/hosting.toml.
        #[arg(long)]
        addr: Option<String>,
        /// Run in watch/reload development mode instead of one-shot serve mode.
        #[arg(long)]
        dev: bool,
        /// Load env vars from a .env-format file.
        #[arg(long = "env")]
        env_files: Vec<PathBuf>,
    },
}

pub fn main_entry() -> ExitCode {
    match try_main() {
        Ok(code) => ExitCode::from(code),
        Err(err) => {
            eprintln!("[agentic-harness] {err}");
            ExitCode::from(1)
        }
    }
}

fn try_main() -> Result<u8, Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        None => default_wizard_command(),
        Some(Commands::Build {
            workspace,
            output,
            target,
            worker_app,
            worker_wasm,
            worker_wasm_crate,
            env_files,
        }) => {
            let target = BuildTarget::parse(&target)?;
            match target {
                BuildTarget::Cloudflare => build_cloudflare(
                    &workspace,
                    output.as_deref(),
                    worker_app.as_deref(),
                    worker_wasm.as_deref(),
                    worker_wasm_crate.as_deref(),
                    &env_files,
                ),
                BuildTarget::Native | BuildTarget::NodeCompat => {
                    if worker_app.is_some() {
                        return Err(
                            "--worker-app is only supported with --target cloudflare".into()
                        );
                    }
                    if worker_wasm.is_some() {
                        return Err(
                            "--worker-wasm is only supported with --target cloudflare".into()
                        );
                    }
                    if worker_wasm_crate.is_some() {
                        return Err(
                            "--worker-wasm-crate is only supported with --target cloudflare".into(),
                        );
                    }
                    build_native(&workspace, output.as_deref(), &env_files, target)
                }
            }
        }
        Some(Commands::Dev {
            workspace,
            target,
            port,
            env_files,
        }) => {
            BuildTarget::parse(&target)?.ensure_native_invocation("dev")?;
            run_dev_server(&workspace, port, &env_files)
        }
        Some(Commands::New {
            path,
            name,
            template,
        }) => {
            scaffold_any_template(&path, name, &template)?;
            Ok(0)
        }
        Some(Commands::Code {
            workspace,
            prompt,
            id,
            test_commands,
            apply_patches,
            llm,
            no_tests,
            commit,
            pr,
            summary,
            summary_json,
        }) => code_command(CodeCommandOptions {
            workspace: &workspace,
            prompt,
            id: &id,
            test_commands: &test_commands,
            apply_patches: &apply_patches,
            llm: llm.as_deref(),
            no_tests,
            commit: commit.as_deref(),
            pr,
            summary: summary.as_deref(),
            summary_json: summary_json.as_deref(),
        }),
        Some(Commands::Result { workspace, json }) => result_command(&workspace, json),
        Some(Commands::Template { command }) => template_command(command),
        Some(Commands::Setup { command }) => setup_command(command),
        Some(Commands::Hosting { command }) => hosting_command(command),
        Some(Commands::Host {
            workspace,
            addr,
            dev,
            env_files,
        }) => host_command(&workspace, addr.as_deref(), dev, &env_files),
        Some(Commands::Sandbox { command }) => sandbox_command(command),
        Some(Commands::Doctor {
            workspace,
            plain,
            json,
        }) => doctor_command(&workspace, plain, json),
        Some(Commands::Dashboard {
            workspace,
            plain,
            json,
        }) => dashboard_command(&workspace, plain, json),
        Some(Commands::Guide {
            workspace,
            env,
            template,
            prompt,
            json,
        }) => guide_command(&workspace, &env, &template, &prompt, json),
        Some(Commands::Smoke { workspace, json }) => smoke_command(&workspace, json),
        Some(Commands::ReleaseCheck { root, json }) => release_check_command(&root, json),
        Some(Commands::Package {
            output,
            binary,
            json,
        }) => package_command(&output, binary.as_deref(), json),
        Some(Commands::Manifest {
            workspace,
            cloudflare,
        }) => run_cargo(
            &workspace,
            [if cloudflare {
                "--agentic-harness-cloudflare-manifest".to_string()
            } else {
                "--agentic-harness-manifest".to_string()
            }],
            &[],
        ),
        Some(Commands::Run {
            agent,
            workspace,
            target,
            output: _,
            id,
            payload,
            env_files,
        }) => {
            BuildTarget::parse(&target)?.ensure_native_invocation("run")?;
            serde_json::from_str::<serde_json::Value>(&payload)
                .map_err(|err| format!("Invalid JSON for --payload: {err}"))?;
            run_cargo(
                &workspace,
                [
                    "--agentic-harness-run".to_string(),
                    agent,
                    "--id".to_string(),
                    id,
                    "--payload".to_string(),
                    payload,
                ],
                &env_files,
            )
        }
        Some(Commands::Serve {
            workspace,
            addr,
            env_files,
        }) => run_cargo(
            &workspace,
            [
                "--agentic-harness-serve".to_string(),
                "--addr".to_string(),
                addr,
            ],
            &env_files,
        ),
        Some(Commands::Add {
            name,
            category,
            print,
        }) => add_command(name, category, print),
        Some(Commands::Wizard { workspace, plain }) => wizard_command(&workspace, plain),
    }
}

fn default_wizard_command() -> Result<u8, Box<dyn std::error::Error>> {
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        wizard_command(Path::new("."), false)
    } else {
        wizard_command(Path::new("."), true)
    }
}

struct CodeCommandOptions<'a> {
    workspace: &'a Path,
    prompt: Option<String>,
    id: &'a str,
    test_commands: &'a [String],
    apply_patches: &'a [PathBuf],
    llm: Option<&'a str>,
    no_tests: bool,
    commit: Option<&'a str>,
    pr: bool,
    summary: Option<&'a Path>,
    summary_json: Option<&'a Path>,
}

const DEFAULT_CODING_SUMMARY_PATH: &str = ".agentic-harness/runs/latest.md";
const DEFAULT_CODING_SUMMARY_JSON_PATH: &str = ".agentic-harness/runs/latest.json";

fn code_command(options: CodeCommandOptions<'_>) -> Result<u8, Box<dyn std::error::Error>> {
    let CodeCommandOptions {
        workspace,
        prompt,
        id,
        test_commands,
        apply_patches,
        llm,
        no_tests,
        commit,
        pr,
        summary,
        summary_json,
    } = options;
    let llm_environment = llm.map(LlmAuthoringEnvironment::parse).transpose()?;
    if !workspace.join("Cargo.toml").exists() {
        if io::stdin().is_terminal() && io::stdout().is_terminal() {
            println!(
                "{ORANGE}!{RESET} No Agentic Harness project found at {}.",
                workspace.display()
            );
            if prompt_yes_no("Create a coding template here", true)? {
                scaffold(workspace, None, ScaffoldTemplate::Coding)?;
            } else {
                return Ok(1);
            }
        } else {
            return Err(format!(
                "No Agentic Harness project found at {}. Create one with `agentic-harness new {} --template coding`.",
                workspace.display(),
                workspace.display()
            )
            .into());
        }
    }

    let report = doctor_report(workspace);
    if !report.required_ready() {
        eprint!("{}", format_doctor_report(&report, false));
        return Ok(1);
    }
    if !llm_authoring_installed(workspace) {
        eprintln!(
            "[agentic-harness] LLM authoring is not installed. Run `agentic-harness setup llm --workspace {}` when you want an LLM to author templates.",
            workspace.display()
        );
    }

    let workspace = workspace.canonicalize()?;
    let sandbox_config = load_sandbox_config(&workspace);
    let prompt = match prompt {
        Some(prompt) => prompt,
        None if io::stdin().is_terminal() && io::stdout().is_terminal() => {
            prompt_required("Coding prompt")?
        }
        None => default_coding_prompt().to_string(),
    };
    coding_progress("inspect", &workspace.display().to_string());
    let git_status_before = git_status_short(&workspace);
    let git_diff_stat = git_diff_stat(&workspace);
    let git_changed_files = git_changed_files(&workspace);
    let root_files = list_root_files(&workspace)?;
    let project_files = detect_project_files(&workspace);
    let workspace_instructions = load_workspace_instructions(&workspace)?;
    let checks = coding_checks_for_workspace(&workspace, test_commands, no_tests);
    let planned_steps = coding_plan_steps(&checks, apply_patches, llm_environment);
    coding_progress("plan", &format!("{} steps", planned_steps.len()));
    let payload = serde_json::json!({
        "repo": ".",
        "prompt": prompt,
        "gitStatusBefore": git_status_before,
        "gitDiffStat": git_diff_stat,
        "gitChangedFiles": git_changed_files,
        "rootFiles": root_files,
        "projectFiles": project_files,
        "workspaceInstructions": workspace_instruction_json_entries(&workspace_instructions),
        "checks": checks,
        "plannedSteps": planned_steps,
    })
    .to_string();

    coding_progress("agent", id);
    let agent_output = run_cargo_capture(
        &workspace,
        [
            "--agentic-harness-run".to_string(),
            "code".to_string(),
            "--id".to_string(),
            id.to_string(),
            "--payload".to_string(),
            payload,
        ],
        &[],
    )?;
    io::stdout().write_all(&agent_output.stdout)?;
    io::stderr().write_all(&agent_output.stderr)?;
    let change_baseline = workspace_file_snapshot(&workspace)?;

    let mut patch_results = apply_patches
        .iter()
        .map(|path| {
            coding_progress("patch", &path.display().to_string());
            apply_coding_patch(&workspace, path)
        })
        .collect::<Vec<_>>();
    for path in write_agent_generated_patches(&agent_output)? {
        coding_progress("patch", &path.display().to_string());
        patch_results.push(apply_coding_patch(&workspace, &path));
    }
    let mut patch_failed = patch_results.iter().any(|result| !result.success);
    let llm_result = if let Some(environment) = llm_environment {
        let brief_path = coding_run_dir(&workspace, id).join("coding-brief.md");
        write_coding_llm_brief(
            &brief_path,
            &CodingLlmBrief {
                workspace: &workspace,
                id,
                prompt: &prompt,
                environment,
                git_status_before: &git_status_before,
                git_diff_stat: &git_diff_stat,
                git_changed_files: &git_changed_files,
                root_files: &root_files,
                project_files: &project_files,
                workspace_instructions: &workspace_instructions,
                checks: &checks,
                planned_steps: &planned_steps,
                agent_output: &agent_output,
                patches: &patch_results,
            },
        )?;
        coding_progress("llm", environment.slug());
        let llm_change_baseline = workspace_file_snapshot(&workspace)?;
        let mut result = run_llm_coding_tool(&workspace, environment, &brief_path);
        result.changed_files =
            changed_files_from_workspace_snapshot(&workspace, &llm_change_baseline)?;
        if !result.stdout.is_empty() {
            io::stdout().write_all(result.stdout.as_bytes())?;
        }
        if !result.stderr.is_empty() {
            io::stderr().write_all(result.stderr.as_bytes())?;
        }
        Some(result)
    } else {
        None
    };
    let llm_failed = llm_result.as_ref().is_some_and(|result| !result.success);
    let mut check_results = if patch_failed || llm_failed {
        Vec::new()
    } else {
        run_coding_checks(&workspace, &sandbox_config, &checks)
    };
    let agent_code = agent_output.status.code().unwrap_or(1) as u8;
    let mut checks_failed = check_results.iter().any(|result| !result.success);
    if agent_code == 0 && !patch_failed && !llm_failed && checks_failed {
        let failed_count = check_results
            .iter()
            .filter(|result| !result.success)
            .count();
        coding_progress(
            "repair",
            &format!(
                "{failed_count} failed check{}",
                if failed_count == 1 { "" } else { "s" }
            ),
        );
        let repair_payload = serde_json::json!({
            "repo": ".",
            "prompt": prompt,
            "gitStatusBefore": git_status_before,
            "gitDiffStat": git_diff_stat,
            "gitChangedFiles": git_changed_files,
            "rootFiles": root_files,
            "projectFiles": project_files,
            "workspaceInstructions": workspace_instruction_json_entries(&workspace_instructions),
            "checks": checks,
            "plannedSteps": planned_steps,
            "repairAttempt": 1,
            "failedChecks": check_results.iter().filter(|result| !result.success).map(|result| {
                serde_json::json!({
                    "command": result.command,
                    "exitCode": result.exit_code,
                    "stdout": result.stdout,
                    "stderr": result.stderr,
                })
            }).collect::<Vec<_>>(),
        })
        .to_string();
        let repair_output = run_cargo_capture(
            &workspace,
            [
                "--agentic-harness-run".to_string(),
                "code".to_string(),
                "--id".to_string(),
                format!("{id}-repair"),
                "--payload".to_string(),
                repair_payload,
            ],
            &[],
        )?;
        io::stderr().write_all(&repair_output.stderr)?;
        if repair_output.status.success() {
            for path in write_agent_generated_patches(&repair_output)? {
                coding_progress("patch", &path.display().to_string());
                patch_results.push(apply_coding_patch(&workspace, &path));
            }
            patch_failed = patch_results.iter().any(|result| !result.success);
            if !patch_failed {
                check_results = run_coding_checks(&workspace, &sandbox_config, &checks);
                checks_failed = check_results.iter().any(|result| !result.success);
            }
        }
    }
    let commit_result = if agent_code == 0 && !patch_failed && !llm_failed && !checks_failed {
        commit.map(|message| {
            coding_progress("commit", message);
            commit_coding_run(&workspace, message)
        })
    } else {
        None
    };
    let commit_failed = commit_result.as_ref().is_some_and(|result| !result.success);
    let pr_result = if agent_code == 0
        && !patch_failed
        && !llm_failed
        && !checks_failed
        && !commit_failed
        && pr
    {
        coding_progress("pr", "gh pr create --fill");
        Some(create_coding_pull_request(&workspace))
    } else {
        None
    };
    let snapshot_changed_files =
        changed_files_from_workspace_snapshot(&workspace, &change_baseline)?;
    let git_status_after = git_status_short(&workspace);
    let changed_files =
        changed_files_from_coding_run(&git_status_after, &patch_results, &snapshot_changed_files);

    let write_default_summary = summary.is_none() && summary_json.is_none();
    if write_default_summary || summary.is_some() || summary_json.is_some() {
        let coding_summary = CodingSummary {
            workspace: workspace.clone(),
            id: id.to_string(),
            prompt: prompt.clone(),
            git_status_before,
            git_status_after,
            git_diff_stat,
            git_changed_files,
            root_files,
            project_files,
            changed_files,
            workspace_instructions: &workspace_instructions,
            planned_steps: &planned_steps,
            sandbox: &sandbox_config,
            agent_output: &agent_output,
            llm: llm_result.as_ref(),
            patches: &patch_results,
            commit: commit_result.as_ref(),
            pull_request: pr_result.as_ref(),
            checks: &check_results,
        };
        if write_default_summary {
            coding_progress("summary", DEFAULT_CODING_SUMMARY_PATH);
            write_coding_summary(
                &workspace.join(DEFAULT_CODING_SUMMARY_PATH),
                &coding_summary,
            )?;
            coding_progress("summary-json", DEFAULT_CODING_SUMMARY_JSON_PATH);
            write_coding_summary_json(
                &workspace.join(DEFAULT_CODING_SUMMARY_JSON_PATH),
                &coding_summary,
            )?;
        }
        if let Some(summary) = summary {
            coding_progress("summary", &summary.display().to_string());
            let path = resolve_workspace_output_path(&workspace, summary);
            write_coding_summary(&path, &coding_summary)?;
        }
        if let Some(summary_json) = summary_json {
            coding_progress("summary-json", &summary_json.display().to_string());
            let path = resolve_workspace_output_path(&workspace, summary_json);
            write_coding_summary_json(&path, &coding_summary)?;
        }
    }

    if agent_code != 0 {
        return Ok(agent_code);
    }
    if patch_failed {
        return Ok(1);
    }
    if llm_failed {
        return Ok(1);
    }
    if checks_failed {
        return Ok(1);
    }
    if commit_failed {
        return Ok(1);
    }
    if pr_result.as_ref().is_some_and(|result| !result.success) {
        return Ok(1);
    }
    Ok(0)
}

fn coding_progress(stage: &str, detail: &str) {
    eprintln!("[agentic-harness] {stage}: {detail}");
}

fn default_coding_prompt() -> &'static str {
    "Inspect the repository, plan the smallest safe coding step, run the available checks, and summarize the result."
}

#[derive(Debug)]
struct CodingCheckResult {
    command: String,
    success: bool,
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CodingPatchResult {
    path: PathBuf,
    success: bool,
    files: Vec<String>,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CodingLlmResult {
    environment: LlmAuthoringEnvironment,
    command: String,
    brief_path: PathBuf,
    success: bool,
    exit_code: Option<i32>,
    changed_files: Vec<String>,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CodingCommitResult {
    message: String,
    success: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct CodingPullRequestResult {
    success: bool,
    stdout: String,
    stderr: String,
}

#[derive(Debug)]
struct WorkspaceInstruction {
    path: String,
    content: String,
}

#[derive(Debug)]
struct CodingSummary<'a> {
    workspace: PathBuf,
    id: String,
    prompt: String,
    git_status_before: String,
    git_status_after: String,
    git_diff_stat: String,
    git_changed_files: Vec<String>,
    root_files: Vec<String>,
    project_files: Vec<String>,
    changed_files: Vec<String>,
    workspace_instructions: &'a [WorkspaceInstruction],
    planned_steps: &'a [String],
    sandbox: &'a SandboxConfig,
    agent_output: &'a Output,
    llm: Option<&'a CodingLlmResult>,
    patches: &'a [CodingPatchResult],
    commit: Option<&'a CodingCommitResult>,
    pull_request: Option<&'a CodingPullRequestResult>,
    checks: &'a [CodingCheckResult],
}

struct CodingLoopEntry {
    phase: &'static str,
    status: String,
    detail: String,
}

fn coding_checks_for_workspace(
    workspace: &Path,
    test_commands: &[String],
    no_tests: bool,
) -> Vec<String> {
    if no_tests {
        return Vec::new();
    }
    let explicit = test_commands
        .iter()
        .map(|command| command.trim())
        .filter(|command| !command.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if !explicit.is_empty() {
        return explicit;
    }

    let mut checks = Vec::new();
    if workspace.join("Cargo.toml").exists() {
        checks.push("cargo test".to_string());
    }
    if workspace.join("package.json").exists() {
        if workspace.join("pnpm-lock.yaml").exists() {
            checks.push("pnpm test".to_string());
        } else if workspace.join("yarn.lock").exists() {
            checks.push("yarn test".to_string());
        } else {
            checks.push("npm test".to_string());
        }
    }
    if workspace.join("pyproject.toml").exists()
        || workspace.join("pytest.ini").exists()
        || workspace.join("tox.ini").exists()
    {
        checks.push("python -m pytest".to_string());
    }
    checks
}

fn coding_plan_steps(
    checks: &[String],
    apply_patches: &[PathBuf],
    llm_environment: Option<LlmAuthoringEnvironment>,
) -> Vec<String> {
    let mut steps = vec![
        "Inspect repository state, root files, and workspace instructions.".to_string(),
        "Run the code agent with the prepared prompt and repository context.".to_string(),
    ];
    if !apply_patches.is_empty() {
        steps.push(format!(
            "Apply prepared patches: {}.",
            apply_patches
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(environment) = llm_environment {
        steps.push(format!(
            "Launch {} with a generated coding brief and let it edit files directly in this workspace.",
            environment.label()
        ));
    }
    if checks.is_empty() {
        steps.push("Skip checks because this run was configured with no checks.".to_string());
    } else {
        steps.push(format!("Run checks: {}.", checks.join("; ")));
    }
    steps.push("Summarize repository changes, check results, and remaining risks.".to_string());
    steps
}

fn list_root_files(workspace: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(workspace)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if matches!(name.as_str(), ".git" | "target") {
            continue;
        }
        let suffix = if entry.file_type()?.is_dir() { "/" } else { "" };
        files.push(format!("{name}{suffix}"));
    }
    files.sort();
    files.truncate(60);
    Ok(files)
}

fn detect_project_files(workspace: &Path) -> Vec<String> {
    [
        "Cargo.toml",
        "Cargo.lock",
        "package.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "package-lock.json",
        "pyproject.toml",
        "requirements.txt",
        "go.mod",
        "Makefile",
        "justfile",
        "Taskfile.yml",
    ]
    .into_iter()
    .filter(|relative| workspace.join(relative).exists())
    .map(ToOwned::to_owned)
    .collect()
}

fn load_workspace_instructions(
    workspace: &Path,
) -> Result<Vec<WorkspaceInstruction>, Box<dyn std::error::Error>> {
    let mut instructions = Vec::new();
    for relative in ["AGENTS.md", "CLAUDE.md"] {
        let path = workspace.join(relative);
        if path.exists() {
            instructions.push(WorkspaceInstruction {
                path: relative.to_string(),
                content: trim_for_payload(&fs::read_to_string(path)?),
            });
        }
    }
    Ok(instructions)
}

fn workspace_instruction_json_entries(
    instructions: &[WorkspaceInstruction],
) -> Vec<serde_json::Value> {
    instructions
        .iter()
        .map(|instruction| {
            serde_json::json!({
                "path": instruction.path,
                "content": instruction.content,
            })
        })
        .collect()
}

fn trim_for_payload(text: &str) -> String {
    const LIMIT: usize = 8_000;
    if text.len() <= LIMIT {
        text.to_string()
    } else {
        let prefix = text.chars().take(LIMIT).collect::<String>();
        format!("{prefix}...\n[truncated]")
    }
}

fn git_status_short(workspace: &Path) -> String {
    git_output(workspace, &["status", "--short"], "git status")
}

fn git_diff_stat(workspace: &Path) -> String {
    git_output(workspace, &["diff", "--stat"], "git diff --stat")
}

fn git_changed_files(workspace: &Path) -> Vec<String> {
    let output = git_output(workspace, &["diff", "--name-only"], "git diff --name-only");
    if output.starts_with("git diff --name-only failed:")
        || output.starts_with("git diff --name-only unavailable:")
    {
        Vec::new()
    } else {
        output
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    }
}

fn git_output(workspace: &Path, args: &[&str], label: &str) -> String {
    match Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
    {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string(),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr_lower = stderr.to_ascii_lowercase();
            let detail = if stderr_lower.contains("not a git repository") {
                "not a git repository"
            } else {
                stderr.trim()
            };
            format!("{label} failed: {detail}")
        }
        Err(err) => format!("{label} unavailable: {err}"),
    }
}

fn changed_files_from_git_status(status: &str) -> Vec<String> {
    if status.starts_with("git status failed:") || status.starts_with("git status unavailable:") {
        return Vec::new();
    }
    let mut files = Vec::new();
    for line in status.lines() {
        if line.len() < 4 {
            continue;
        }
        let mut file = line[3..].trim();
        if let Some((_, renamed_to)) = file.rsplit_once(" -> ") {
            file = renamed_to.trim();
        }
        let file = file.trim_matches('"');
        if file.is_empty() || files.iter().any(|existing| existing == file) {
            continue;
        }
        files.push(file.to_string());
    }
    files
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WorkspaceFileFingerprint {
    len: u64,
    hash: u64,
}

fn workspace_file_snapshot(
    workspace: &Path,
) -> Result<BTreeMap<String, WorkspaceFileFingerprint>, Box<dyn std::error::Error>> {
    let mut files = BTreeMap::new();
    collect_workspace_file_snapshot(workspace, workspace, &mut files)?;
    Ok(files)
}

fn collect_workspace_file_snapshot(
    workspace: &Path,
    current: &Path,
    files: &mut BTreeMap<String, WorkspaceFileFingerprint>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(workspace).unwrap_or(&path);
        let file_type = entry.file_type()?;
        if should_skip_workspace_snapshot_path(relative) {
            continue;
        }
        if file_type.is_dir() {
            collect_workspace_file_snapshot(workspace, &path, files)?;
        } else if file_type.is_file() {
            files.insert(
                normalize_relative_path(relative),
                workspace_file_fingerprint(&path)?,
            );
        }
    }
    Ok(())
}

fn should_skip_workspace_snapshot_path(relative: &Path) -> bool {
    let parts = relative
        .iter()
        .filter_map(|part| part.to_str())
        .collect::<Vec<_>>();
    if parts
        .iter()
        .any(|part| matches!(*part, ".git" | "target" | "node_modules" | ".next"))
    {
        return true;
    }
    matches!(parts.as_slice(), [".agentic-harness", "runs", ..])
}

fn normalize_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

fn workspace_file_fingerprint(
    path: &Path,
) -> Result<WorkspaceFileFingerprint, Box<dyn std::error::Error>> {
    let content = fs::read(path)?;
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    Ok(WorkspaceFileFingerprint {
        len: content.len() as u64,
        hash: hasher.finish(),
    })
}

fn changed_files_from_workspace_snapshot(
    workspace: &Path,
    before: &BTreeMap<String, WorkspaceFileFingerprint>,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let after = workspace_file_snapshot(workspace)?;
    let mut files = Vec::new();
    for (path, fingerprint) in &after {
        if before.get(path) != Some(fingerprint) {
            files.push(path.clone());
        }
    }
    for path in before.keys() {
        if !after.contains_key(path) {
            files.push(path.clone());
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

fn changed_files_from_coding_run(
    status: &str,
    patches: &[CodingPatchResult],
    snapshot_files: &[String],
) -> Vec<String> {
    let mut files = changed_files_from_git_status(status);
    for patch in patches {
        for file in &patch.files {
            if !files.iter().any(|existing| existing == file) {
                files.push(file.clone());
            }
        }
    }
    for file in snapshot_files {
        if !files.iter().any(|existing| existing == file) {
            files.push(file.clone());
        }
    }
    files
}

fn run_coding_check(workspace: &Path, config: &SandboxConfig, command: &str) -> CodingCheckResult {
    if config.target != "local" {
        return match remote_sandbox_env(config).and_then(|env| {
            env.exec(command, ShellOptions::new())
                .map_err(|err| -> Box<dyn std::error::Error> { Box::new(err) })
        }) {
            Ok(output) => CodingCheckResult {
                command: command.to_string(),
                success: output.exit_code == 0,
                exit_code: Some(output.exit_code),
                stdout: output.stdout,
                stderr: output.stderr,
            },
            Err(err) => CodingCheckResult {
                command: command.to_string(),
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: err.to_string(),
            },
        };
    }

    match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(workspace)
        .output()
    {
        Ok(output) => CodingCheckResult {
            command: command.to_string(),
            success: output.status.success(),
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(err) => CodingCheckResult {
            command: command.to_string(),
            success: false,
            exit_code: None,
            stdout: String::new(),
            stderr: err.to_string(),
        },
    }
}

fn run_coding_checks(
    workspace: &Path,
    config: &SandboxConfig,
    checks: &[String],
) -> Vec<CodingCheckResult> {
    if checks.is_empty() {
        return Vec::new();
    }
    if config.target != "local" {
        coding_progress(
            "sync",
            &format!("{} -> {}", workspace.display(), config.cwd),
        );
        if let Err(err) = sync_coding_workspace_to_remote_sandbox(workspace, config) {
            return vec![CodingCheckResult {
                command: "sync remote sandbox".to_string(),
                success: false,
                exit_code: None,
                stdout: String::new(),
                stderr: err.to_string(),
            }];
        }
    }
    checks
        .iter()
        .map(|command| {
            coding_progress("check", command);
            run_coding_check(workspace, config, command)
        })
        .collect()
}

fn apply_coding_patch(workspace: &Path, path: &Path) -> CodingPatchResult {
    let patch_path = resolve_workspace_output_path(workspace, path);
    let files = patch_changed_files(&patch_path);
    match Command::new("git")
        .arg("apply")
        .arg("--whitespace=nowarn")
        .arg(&patch_path)
        .current_dir(workspace)
        .output()
    {
        Ok(output) => CodingPatchResult {
            path: patch_path,
            success: output.status.success(),
            files,
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(err) => CodingPatchResult {
            path: patch_path,
            success: false,
            files,
            stdout: String::new(),
            stderr: err.to_string(),
        },
    }
}

struct CodingLlmBrief<'a> {
    workspace: &'a Path,
    id: &'a str,
    prompt: &'a str,
    environment: LlmAuthoringEnvironment,
    git_status_before: &'a str,
    git_diff_stat: &'a str,
    git_changed_files: &'a [String],
    root_files: &'a [String],
    project_files: &'a [String],
    workspace_instructions: &'a [WorkspaceInstruction],
    checks: &'a [String],
    planned_steps: &'a [String],
    agent_output: &'a Output,
    patches: &'a [CodingPatchResult],
}

fn coding_run_dir(workspace: &Path, id: &str) -> PathBuf {
    workspace
        .join(".agentic-harness/runs")
        .join(sanitize_coding_run_id(id))
}

fn sanitize_coding_run_id(id: &str) -> String {
    let sanitized = id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    let trimmed = sanitized.trim_matches(&['.', '-'][..]);
    if trimmed.is_empty() {
        "run".to_string()
    } else {
        trimmed.to_string()
    }
}

fn write_coding_llm_brief(
    path: &Path,
    brief: &CodingLlmBrief<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, render_coding_llm_brief(brief))?;
    eprintln!("[agentic-harness] wrote coding brief to {}", path.display());
    Ok(())
}

fn render_coding_llm_brief(brief: &CodingLlmBrief<'_>) -> String {
    let mut out = String::new();
    out.push_str("# Agentic Harness Coding Brief\n\n");
    out.push_str(&format!("Workspace: `{}`\n", brief.workspace.display()));
    out.push_str(&format!("Request id: `{}`\n", brief.id));
    out.push_str(&format!("Target LLM: {}\n", brief.environment.label()));
    out.push_str(&format!("Prompt: {}\n\n", brief.prompt));

    out.push_str("## Required Loop\n\n");
    out.push_str("1. Inspect the repository context below before editing.\n");
    out.push_str("2. Make the smallest focused code changes that satisfy the prompt.\n");
    out.push_str("3. Edit files directly in this workspace.\n");
    out.push_str("4. Leave the workspace ready for the listed checks.\n");
    out.push_str(
        "5. Do not commit or open a pull request unless the outer CLI run requested it.\n",
    );

    out.push_str("\n## Planned Steps\n\n");
    for step in brief.planned_steps {
        out.push_str(&format!("- {step}\n"));
    }

    out.push_str("\n## Checks To Satisfy\n\n");
    if brief.checks.is_empty() {
        out.push_str("- none\n");
    } else {
        for check in brief.checks {
            out.push_str(&format!("- `{check}`\n"));
        }
    }

    out.push_str("\n## Root Files\n\n");
    for file in brief.root_files {
        out.push_str(&format!("- `{file}`\n"));
    }

    out.push_str("\n## Project Files\n\n");
    if brief.project_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for file in brief.project_files {
            out.push_str(&format!("- `{file}`\n"));
        }
    }

    out.push_str("\n## Git State Before Run\n\n");
    out.push_str("Changed files:\n");
    if brief.git_changed_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for file in brief.git_changed_files {
            out.push_str(&format!("- `{file}`\n"));
        }
    }
    out.push_str("\nStatus:\n");
    push_text_block(&mut out, brief.git_status_before);
    out.push_str("\nDiff stat:\n");
    push_text_block(&mut out, brief.git_diff_stat);

    out.push_str("\n## Workspace Instructions\n\n");
    if brief.workspace_instructions.is_empty() {
        out.push_str("No AGENTS.md or CLAUDE.md instructions were found.\n");
    } else {
        for instruction in brief.workspace_instructions {
            out.push_str(&format!("### `{}`\n\n", instruction.path));
            push_text_block(&mut out, &trim_for_summary(&instruction.content));
        }
    }

    out.push_str("\n## Native Agent Output\n\n");
    push_text_block(
        &mut out,
        &trim_for_summary(&String::from_utf8_lossy(&brief.agent_output.stdout)),
    );

    out.push_str("\n## Patches Already Applied\n\n");
    if brief.patches.is_empty() {
        out.push_str("- none\n");
    } else {
        for patch in brief.patches {
            out.push_str(&format!(
                "- `{}`: {}\n",
                patch.path.display(),
                if patch.success { "applied" } else { "failed" }
            ));
        }
    }
    out
}

fn run_llm_coding_tool(
    workspace: &Path,
    environment: LlmAuthoringEnvironment,
    brief_path: &Path,
) -> CodingLlmResult {
    let invocation = match llm_coding_invocation(environment, brief_path) {
        Ok(invocation) => invocation,
        Err(err) => {
            return CodingLlmResult {
                environment,
                command: llm_authoring_command_name(environment).to_string(),
                brief_path: brief_path.to_path_buf(),
                success: false,
                exit_code: None,
                changed_files: Vec::new(),
                stdout: String::new(),
                stderr: err.to_string(),
            };
        }
    };
    match run_llm_invocation_with_echo(workspace, &invocation, LlmInvocationEcho::Stderr) {
        Ok(output) => CodingLlmResult {
            environment,
            command: invocation.command,
            brief_path: brief_path.to_path_buf(),
            success: output.status.success(),
            exit_code: output.status.code(),
            changed_files: Vec::new(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(err) => CodingLlmResult {
            environment,
            command: invocation.command,
            brief_path: brief_path.to_path_buf(),
            success: false,
            exit_code: None,
            changed_files: Vec::new(),
            stdout: String::new(),
            stderr: err.to_string(),
        },
    }
}

struct LlmCodingInvocation {
    command: String,
    args: Vec<String>,
    stdin: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LlmInvocationEcho {
    StdoutStderr,
    Stderr,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LlmStreamEcho {
    Stdout,
    Stderr,
}

fn llm_coding_invocation(
    environment: LlmAuthoringEnvironment,
    brief_path: &Path,
) -> Result<LlmCodingInvocation, Box<dyn std::error::Error>> {
    llm_prompt_invocation(environment, &fs::read_to_string(brief_path)?, brief_path)
}

fn llm_prompt_invocation(
    environment: LlmAuthoringEnvironment,
    prompt: &str,
    brief_path: &Path,
) -> Result<LlmCodingInvocation, Box<dyn std::error::Error>> {
    let command = llm_authoring_command_name(environment).to_string();
    let invocation = match environment {
        LlmAuthoringEnvironment::ClaudeCode => LlmCodingInvocation {
            command,
            args: vec![
                "-p".to_string(),
                "--permission-mode".to_string(),
                "acceptEdits".to_string(),
                prompt.to_string(),
            ],
            stdin: None,
        },
        LlmAuthoringEnvironment::Codex => LlmCodingInvocation {
            command,
            args: vec![
                "exec".to_string(),
                "--sandbox".to_string(),
                "workspace-write".to_string(),
                "--skip-git-repo-check".to_string(),
                "-".to_string(),
            ],
            stdin: Some(prompt.to_string()),
        },
        LlmAuthoringEnvironment::Cursor => LlmCodingInvocation {
            command,
            args: vec![
                "agent".to_string(),
                "--print".to_string(),
                "--trust".to_string(),
                "--force".to_string(),
                prompt.to_string(),
            ],
            stdin: None,
        },
        LlmAuthoringEnvironment::WindServer => LlmCodingInvocation {
            command,
            args: vec![brief_path.display().to_string()],
            stdin: None,
        },
    };
    Ok(invocation)
}

fn run_llm_invocation_with_echo(
    workspace: &Path,
    invocation: &LlmCodingInvocation,
    echo: LlmInvocationEcho,
) -> Result<Output, Box<dyn std::error::Error>> {
    let mut child = Command::new(&invocation.command)
        .args(&invocation.args)
        .current_dir(workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("failed to open LLM stdout")?;
    let stderr = child.stderr.take().ok_or("failed to open LLM stderr")?;
    let stdout_handle = capture_child_stream(
        stdout,
        match echo {
            LlmInvocationEcho::StdoutStderr => LlmStreamEcho::Stdout,
            LlmInvocationEcho::Stderr => LlmStreamEcho::Stderr,
        },
    );
    let stderr_handle = capture_child_stream(
        stderr,
        match echo {
            LlmInvocationEcho::StdoutStderr | LlmInvocationEcho::Stderr => LlmStreamEcho::Stderr,
        },
    );
    if let Some(stdin) = &invocation.stdin {
        let mut child_stdin = child
            .stdin
            .take()
            .ok_or("failed to open LLM command stdin")?;
        child_stdin.write_all(stdin.as_bytes())?;
    } else {
        drop(child.stdin.take());
    }
    let status = child.wait()?;
    let stdout = join_captured_stream(stdout_handle)?;
    let stderr = join_captured_stream(stderr_handle)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn capture_child_stream<R: Read + Send + 'static>(
    mut reader: R,
    echo: LlmStreamEcho,
) -> thread::JoinHandle<io::Result<Vec<u8>>> {
    thread::spawn(move || {
        let mut captured = Vec::new();
        let mut buffer = [0_u8; 8192];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let chunk = &buffer[..read];
            captured.extend_from_slice(chunk);
            match echo {
                LlmStreamEcho::Stdout => {
                    io::stdout().write_all(chunk)?;
                    io::stdout().flush()?;
                }
                LlmStreamEcho::Stderr => {
                    io::stderr().write_all(chunk)?;
                    io::stderr().flush()?;
                }
            }
        }
        Ok(captured)
    })
}

fn join_captured_stream(
    handle: thread::JoinHandle<io::Result<Vec<u8>>>,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    match handle.join() {
        Ok(result) => Ok(result?),
        Err(_) => Err("failed to join LLM output reader".into()),
    }
}

fn patch_changed_files(path: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            if let Some(file) = rest
                .split_whitespace()
                .nth(1)
                .and_then(|path| path.strip_prefix("b/"))
            {
                push_unique_patch_file(&mut files, file);
            }
        } else if let Some(file) = line.strip_prefix("+++ b/") {
            push_unique_patch_file(&mut files, file);
        }
    }
    files
}

fn push_unique_patch_file(files: &mut Vec<String>, file: &str) {
    if file.is_empty() || file.starts_with('/') || file.contains("..") {
        return;
    }
    if !files.iter().any(|existing| existing == file) {
        files.push(file.to_string());
    }
}

fn write_agent_generated_patches(
    agent_output: &Output,
) -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&agent_output.stdout) else {
        return Ok(Vec::new());
    };
    let Some(patch) = value.get("generatedPatch").and_then(|patch| patch.as_str()) else {
        return Ok(Vec::new());
    };
    if patch.trim().is_empty() {
        return Ok(Vec::new());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let path = std::env::temp_dir().join("agentic-harness").join(format!(
        "{}-{}-agent-generated.patch",
        std::process::id(),
        stamp
    ));
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, patch)?;
    Ok(vec![path])
}

fn commit_coding_run(workspace: &Path, message: &str) -> CodingCommitResult {
    let add = Command::new("git")
        .args(["add", "-A"])
        .current_dir(workspace)
        .output();
    match add {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            return CodingCommitResult {
                message: message.to_string(),
                success: false,
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            };
        }
        Err(err) => {
            return CodingCommitResult {
                message: message.to_string(),
                success: false,
                stdout: String::new(),
                stderr: err.to_string(),
            };
        }
    }

    match Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(workspace)
        .output()
    {
        Ok(output) => CodingCommitResult {
            message: message.to_string(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(err) => CodingCommitResult {
            message: message.to_string(),
            success: false,
            stdout: String::new(),
            stderr: err.to_string(),
        },
    }
}

fn create_coding_pull_request(workspace: &Path) -> CodingPullRequestResult {
    match Command::new("gh")
        .args(["pr", "create", "--fill"])
        .current_dir(workspace)
        .output()
    {
        Ok(output) => CodingPullRequestResult {
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        },
        Err(err) => CodingPullRequestResult {
            success: false,
            stdout: String::new(),
            stderr: err.to_string(),
        },
    }
}

fn resolve_workspace_output_path(workspace: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        workspace.join(path)
    }
}

fn write_coding_summary(
    path: &Path,
    summary: &CodingSummary<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, render_coding_summary(summary))?;
    eprintln!(
        "[agentic-harness] wrote coding summary to {}",
        path.display()
    );
    Ok(())
}

fn write_coding_summary_json(
    path: &Path,
    summary: &CodingSummary<'_>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, render_coding_summary_json(summary)?)?;
    eprintln!(
        "[agentic-harness] wrote coding summary JSON to {}",
        path.display()
    );
    Ok(())
}

fn render_coding_summary_json(
    summary: &CodingSummary<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let loop_entries = coding_loop_entries(summary);
    let value = serde_json::json!({
        "workspace": summary.workspace.display().to_string(),
        "id": summary.id,
        "prompt": summary.prompt,
        "loop": loop_entries.iter().map(|entry| {
            serde_json::json!({
                "phase": entry.phase,
                "status": entry.status,
                "detail": entry.detail,
            })
        }).collect::<Vec<_>>(),
        "plannedSteps": summary.planned_steps,
        "rootFiles": summary.root_files,
        "projectFiles": summary.project_files,
        "workspaceInstructions": workspace_instruction_json_entries(summary.workspace_instructions),
        "changedFiles": summary.changed_files,
        "gitStatusBefore": summary.git_status_before,
        "gitStatusAfter": summary.git_status_after,
        "gitDiffStat": summary.git_diff_stat,
        "gitChangedFiles": summary.git_changed_files,
        "sandbox": coding_summary_sandbox_json(summary.sandbox),
        "nextCommands": coding_summary_next_commands(summary),
        "agentResult": coding_agent_result_json(summary.agent_output),
        "agent": {
            "status": if summary.agent_output.status.success() { "passed" } else { "failed" },
            "exitCode": summary.agent_output.status.code(),
            "stdout": trim_for_summary(&String::from_utf8_lossy(&summary.agent_output.stdout)),
            "stderr": trim_for_summary(&String::from_utf8_lossy(&summary.agent_output.stderr)),
        },
        "llm": summary.llm.map(|llm| {
            serde_json::json!({
                "environment": llm.environment.slug(),
                "tool": llm.command,
                "brief": llm.brief_path.display().to_string(),
                "status": if llm.success { "completed" } else { "failed" },
                "success": llm.success,
                "exitCode": llm.exit_code,
                "changedFiles": llm.changed_files,
                "stdout": trim_for_summary(&llm.stdout),
                "stderr": trim_for_summary(&llm.stderr),
            })
        }),
        "patches": summary.patches.iter().map(|patch| {
            serde_json::json!({
                "path": patch.path.display().to_string(),
                "status": if patch.success { "applied" } else { "failed" },
                "success": patch.success,
                "files": patch.files,
                "stdout": trim_for_summary(&patch.stdout),
                "stderr": trim_for_summary(&patch.stderr),
            })
        }).collect::<Vec<_>>(),
        "commit": summary.commit.map(|commit| {
            serde_json::json!({
                "message": commit.message,
                "status": if commit.success { "committed" } else { "failed" },
                "success": commit.success,
                "stdout": trim_for_summary(&commit.stdout),
                "stderr": trim_for_summary(&commit.stderr),
            })
        }),
        "pullRequest": summary.pull_request.map(|pr| {
            serde_json::json!({
                "status": if pr.success { "created" } else { "failed" },
                "success": pr.success,
                "stdout": trim_for_summary(&pr.stdout),
                "stderr": trim_for_summary(&pr.stderr),
            })
        }),
        "checks": summary.checks.iter().map(|check| {
            serde_json::json!({
                "command": check.command,
                "status": if check.success { "passed" } else { "failed" },
                "success": check.success,
                "exitCode": check.exit_code,
                "stdout": trim_for_summary(&check.stdout),
                "stderr": trim_for_summary(&check.stderr),
            })
        }).collect::<Vec<_>>(),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn render_coding_summary(summary: &CodingSummary<'_>) -> String {
    let mut out = String::new();
    out.push_str("# Agentic Harness Coding Run\n\n");
    out.push_str(&format!("Workspace: `{}`\n", summary.workspace.display()));
    out.push_str(&format!("Request id: `{}`\n", summary.id));
    out.push_str(&format!("Prompt: {}\n\n", summary.prompt));

    out.push_str("## Coding Loop\n\n");
    for entry in coding_loop_entries(summary) {
        out.push_str(&format!(
            "- {}: {} - {}\n",
            entry.phase, entry.status, entry.detail
        ));
    }
    out.push('\n');

    out.push_str("## Plan\n\n");
    if summary.planned_steps.is_empty() {
        out.push_str("- none\n");
    } else {
        for step in summary.planned_steps {
            out.push_str(&format!("- {step}\n"));
        }
    }

    out.push_str("## Repository Inspect\n\n");
    out.push_str("Root files:\n");
    if summary.root_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for file in &summary.root_files {
            out.push_str(&format!("- `{file}`\n"));
        }
    }

    out.push_str("\n## Project Context\n\n");
    out.push_str("Project files:\n");
    if summary.project_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for file in &summary.project_files {
            out.push_str(&format!("- `{file}`\n"));
        }
    }
    out.push_str("\nGit changed files before run:\n");
    if summary.git_changed_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for file in &summary.git_changed_files {
            out.push_str(&format!("- `{file}`\n"));
        }
    }
    out.push_str("\nGit diff stat:\n");
    push_text_block(&mut out, &summary.git_diff_stat);

    out.push_str("\n## Sandbox\n\n");
    out.push_str(&format!("target: `{}`\n\n", summary.sandbox.target));
    out.push_str(&format!("cwd: `{}`\n\n", summary.sandbox.cwd));
    out.push_str(&format!(
        "endpoint: `{}`\n\n",
        summary.sandbox.endpoint.as_deref().unwrap_or("none")
    ));
    out.push_str(&format!(
        "check mode: `{}`\n",
        sandbox_check_mode(summary.sandbox)
    ));

    out.push_str("\n## Workspace Instructions\n\n");
    if summary.workspace_instructions.is_empty() {
        out.push_str("No workspace instruction files were found.\n");
    } else {
        for instruction in summary.workspace_instructions {
            out.push_str(&format!("### `{}`\n\n", instruction.path));
            push_text_block(&mut out, &trim_for_summary(&instruction.content));
            out.push('\n');
        }
    }

    out.push_str("\n## Repository Status\n\n");
    out.push_str("Git status before:\n");
    push_text_block(&mut out, &summary.git_status_before);
    out.push_str("\nGit status after:\n");
    push_text_block(&mut out, &summary.git_status_after);

    out.push_str("\n## Changed files\n\n");
    if summary.changed_files.is_empty() {
        out.push_str("- none\n");
    } else {
        for file in &summary.changed_files {
            out.push_str(&format!("- `{file}`\n"));
        }
    }

    out.push_str("\n## Next Commands\n\n");
    for command in coding_summary_next_commands(summary) {
        out.push_str(&format!("- `{command}`\n"));
    }

    out.push_str("\n## Agent result\n\n");
    out.push_str(&format!(
        "status: {}\n\n",
        output_status_label(summary.agent_output)
    ));
    out.push_str("\n## Structured agent result\n\n");
    if let Some(result) = coding_agent_result_json(summary.agent_output) {
        if let Some(summary_text) = result.get("summary").and_then(serde_json::Value::as_str) {
            out.push_str(&format!("summary: {summary_text}\n\n"));
        }
        let pretty = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
        push_text_block(&mut out, &trim_for_summary(&pretty));
        out.push('\n');
    } else {
        out.push_str("No structured JSON result was returned by the agent.\n");
    }

    out.push_str("\n## Agent stdout\n\n");
    out.push_str("stdout:\n");
    push_text_block(
        &mut out,
        &trim_for_summary(&String::from_utf8_lossy(&summary.agent_output.stdout)),
    );
    if !summary.agent_output.stderr.is_empty() {
        out.push_str("\nstderr:\n");
        push_text_block(
            &mut out,
            &trim_for_summary(&String::from_utf8_lossy(&summary.agent_output.stderr)),
        );
    }

    out.push_str("\n## LLM coding tool\n\n");
    if let Some(llm) = summary.llm {
        out.push_str(&format!("tool: {}\n\n", llm.command));
        out.push_str(&format!("environment: {}\n\n", llm.environment.slug()));
        out.push_str(&format!("brief: `{}`\n\n", llm.brief_path.display()));
        out.push_str(&format!(
            "status: {}\n\n",
            if llm.success { "completed" } else { "failed" }
        ));
        out.push_str("changed files:\n");
        if llm.changed_files.is_empty() {
            out.push_str("- none\n\n");
        } else {
            for file in &llm.changed_files {
                out.push_str(&format!("- `{file}`\n"));
            }
            out.push('\n');
        }
        if !llm.stdout.is_empty() {
            out.push_str("stdout:\n");
            push_text_block(&mut out, &trim_for_summary(&llm.stdout));
        }
        if !llm.stderr.is_empty() {
            out.push_str("\nstderr:\n");
            push_text_block(&mut out, &trim_for_summary(&llm.stderr));
        }
    } else {
        out.push_str("No external LLM coding tool was requested.\n");
    }

    out.push_str("\n## Applied patches\n\n");
    if summary.patches.is_empty() {
        out.push_str("No patches were applied.\n");
    } else {
        for patch in summary.patches {
            out.push_str(&format!("### Patch: `{}`\n\n", patch.path.display()));
            out.push_str(&format!(
                "status: {}\n\n",
                if patch.success { "applied" } else { "failed" }
            ));
            out.push_str("files:\n");
            if patch.files.is_empty() {
                out.push_str("- unknown\n");
            } else {
                for file in &patch.files {
                    out.push_str(&format!("- `{file}`\n"));
                }
            }
            if !patch.stdout.is_empty() {
                out.push_str("\nstdout:\n");
                push_text_block(&mut out, &trim_for_summary(&patch.stdout));
            }
            if !patch.stderr.is_empty() {
                out.push_str("\nstderr:\n");
                push_text_block(&mut out, &trim_for_summary(&patch.stderr));
            }
            out.push('\n');
        }
    }

    out.push_str("\n## Commit\n\n");
    if let Some(commit) = summary.commit {
        out.push_str(&format!("message: {}\n\n", commit.message));
        out.push_str(&format!(
            "status: {}\n\n",
            if commit.success {
                "committed"
            } else {
                "failed"
            }
        ));
        if !commit.stdout.is_empty() {
            out.push_str("stdout:\n");
            push_text_block(&mut out, &trim_for_summary(&commit.stdout));
        }
        if !commit.stderr.is_empty() {
            out.push_str("\nstderr:\n");
            push_text_block(&mut out, &trim_for_summary(&commit.stderr));
        }
    } else {
        out.push_str("No commit was requested.\n");
    }

    out.push_str("\n## Pull request\n\n");
    if let Some(pr) = summary.pull_request {
        out.push_str(&format!(
            "status: {}\n\n",
            if pr.success { "created" } else { "failed" }
        ));
        if !pr.stdout.is_empty() {
            out.push_str("stdout:\n");
            push_text_block(&mut out, &trim_for_summary(&pr.stdout));
        }
        if !pr.stderr.is_empty() {
            out.push_str("\nstderr:\n");
            push_text_block(&mut out, &trim_for_summary(&pr.stderr));
        }
    } else {
        out.push_str("No pull request was requested.\n");
    }

    out.push_str("\n## Checks\n\n");
    if summary.checks.is_empty() {
        out.push_str("No checks were run.\n");
    } else {
        for check in summary.checks {
            out.push_str(&format!("### Check: `{}`\n\n", check.command));
            out.push_str(&format!(
                "status: {}\n\n",
                if check.success { "passed" } else { "failed" }
            ));
            out.push_str(&format!(
                "exit code: {}\n\n",
                check
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "unavailable".to_string())
            ));
            if !check.stdout.is_empty() {
                out.push_str("stdout:\n");
                push_text_block(&mut out, &trim_for_summary(&check.stdout));
                out.push('\n');
            }
            if !check.stderr.is_empty() {
                out.push_str("stderr:\n");
                push_text_block(&mut out, &trim_for_summary(&check.stderr));
                out.push('\n');
            }
        }
    }
    out
}

fn coding_agent_result_json(output: &Output) -> Option<serde_json::Value> {
    serde_json::from_slice::<serde_json::Value>(&output.stdout).ok()
}

fn coding_summary_sandbox_json(config: &SandboxConfig) -> serde_json::Value {
    serde_json::json!({
        "target": config.target,
        "cwd": config.cwd,
        "endpoint": config.endpoint,
        "checkMode": sandbox_check_mode(config),
    })
}

fn sandbox_check_mode(config: &SandboxConfig) -> &'static str {
    if config.target == "local" {
        "local"
    } else {
        "remote"
    }
}

fn coding_summary_next_commands(summary: &CodingSummary<'_>) -> Vec<String> {
    let workspace_arg = quote_cli_arg(&summary.workspace.display().to_string());
    let mut commands = vec![
        format!("agentic-harness inspect --workspace {workspace_arg}"),
        format!("agentic-harness dashboard --workspace {workspace_arg}"),
    ];
    let mut rerun = format!(
        "agentic-harness code --workspace {workspace_arg} --prompt {}",
        quote_cli_value(&summary.prompt)
    );
    if let Some(llm) = summary.llm {
        rerun.push_str(&format!(" --llm {}", llm.environment.slug()));
    }
    commands.push(rerun);
    commands
}

fn coding_loop_entries(summary: &CodingSummary<'_>) -> Vec<CodingLoopEntry> {
    vec![
        CodingLoopEntry {
            phase: "inspect",
            status: "completed".to_string(),
            detail: format!(
                "{} root entries, {} project markers, {} instruction files",
                summary.root_files.len(),
                summary.project_files.len(),
                summary.workspace_instructions.len()
            ),
        },
        CodingLoopEntry {
            phase: "plan",
            status: "completed".to_string(),
            detail: format!(
                "{} planned step{}",
                summary.planned_steps.len(),
                plural(summary.planned_steps.len())
            ),
        },
        CodingLoopEntry {
            phase: "edit",
            status: coding_edit_status(summary).to_string(),
            detail: coding_edit_detail(summary),
        },
        CodingLoopEntry {
            phase: "test",
            status: coding_test_status(summary).to_string(),
            detail: coding_test_detail(summary),
        },
        CodingLoopEntry {
            phase: "summarize",
            status: "completed".to_string(),
            detail: format!(
                "{} changed file{} recorded",
                summary.changed_files.len(),
                plural(summary.changed_files.len())
            ),
        },
        CodingLoopEntry {
            phase: "commit",
            status: coding_commit_status(summary).to_string(),
            detail: coding_commit_detail(summary),
        },
        CodingLoopEntry {
            phase: "pull-request",
            status: coding_pull_request_status(summary).to_string(),
            detail: coding_pull_request_detail(summary),
        },
    ]
}

fn coding_edit_status(summary: &CodingSummary<'_>) -> &'static str {
    if summary.patches.iter().any(|patch| !patch.success)
        || summary.llm.is_some_and(|llm| !llm.success)
    {
        "failed"
    } else if summary.patches.iter().any(|patch| patch.success) {
        "applied"
    } else if summary.llm.is_some_and(|llm| llm.success) || !summary.changed_files.is_empty() {
        "completed"
    } else {
        "skipped"
    }
}

fn coding_edit_detail(summary: &CodingSummary<'_>) -> String {
    let patch_count = summary.patches.len();
    let changed_count = summary.changed_files.len();
    let patch_word = if patch_count == 1 { "patch" } else { "patches" };
    match summary.llm {
        Some(llm) => format!(
            "{} {}, {} changed file{}, LLM {}",
            patch_count,
            patch_word,
            changed_count,
            plural(changed_count),
            if llm.success { "completed" } else { "failed" }
        ),
        None => format!(
            "{} {}, {} changed file{}",
            patch_count,
            patch_word,
            changed_count,
            plural(changed_count)
        ),
    }
}

fn coding_test_status(summary: &CodingSummary<'_>) -> &'static str {
    if summary.checks.is_empty() {
        "skipped"
    } else if summary.checks.iter().all(|check| check.success) {
        "passed"
    } else {
        "failed"
    }
}

fn coding_test_detail(summary: &CodingSummary<'_>) -> String {
    if summary.checks.is_empty() {
        "no checks configured".to_string()
    } else {
        let passed = summary.checks.iter().filter(|check| check.success).count();
        format!(
            "{passed}/{} check{} passed",
            summary.checks.len(),
            plural(summary.checks.len())
        )
    }
}

fn coding_commit_status(summary: &CodingSummary<'_>) -> &'static str {
    match summary.commit {
        Some(commit) if commit.success => "committed",
        Some(_) => "failed",
        None => "skipped",
    }
}

fn coding_commit_detail(summary: &CodingSummary<'_>) -> String {
    match summary.commit {
        Some(commit) => commit.message.clone(),
        None => "no commit requested".to_string(),
    }
}

fn coding_pull_request_status(summary: &CodingSummary<'_>) -> &'static str {
    match summary.pull_request {
        Some(pr) if pr.success => "created",
        Some(_) => "failed",
        None => "skipped",
    }
}

fn coding_pull_request_detail(summary: &CodingSummary<'_>) -> String {
    match summary.pull_request {
        Some(pr) if pr.stdout.trim().is_empty() => "gh pr create --fill".to_string(),
        Some(pr) => trim_for_summary(&pr.stdout),
        None => "no pull request requested".to_string(),
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn output_status_label(output: &Output) -> String {
    if output.status.success() {
        "passed".to_string()
    } else {
        format!(
            "failed ({})",
            output
                .status
                .code()
                .map(|code| format!("exit {code}"))
                .unwrap_or_else(|| "terminated".to_string())
        )
    }
}

fn push_text_block(out: &mut String, text: &str) {
    out.push_str("```text\n");
    if text.trim().is_empty() {
        out.push_str("(empty)\n");
    } else {
        out.push_str(text.trim_end());
        out.push('\n');
    }
    out.push_str("```\n");
}

fn trim_for_summary(text: &str) -> String {
    const LIMIT: usize = 12_000;
    if text.len() <= LIMIT {
        text.to_string()
    } else {
        let prefix = text.chars().take(LIMIT).collect::<String>();
        format!("{prefix}...\n[truncated]")
    }
}

fn template_command(command: TemplateCommands) -> Result<u8, Box<dyn std::error::Error>> {
    match command {
        TemplateCommands::Init {
            path,
            name,
            agent,
            description,
            version,
        } => {
            init_template_pack(&path, name, agent, &description, &version)?;
            Ok(0)
        }
        TemplateCommands::List {
            workspace,
            verbose,
            json,
        } => {
            if json {
                print!("{}", format_template_list_json(&workspace)?);
                return Ok(0);
            }
            println!("Built-in templates:");
            for template in BUILT_IN_TEMPLATES {
                if verbose {
                    println!(
                        "  - {} (agent {}, version {}, {})",
                        template.name(),
                        template.agent_name(),
                        env!("CARGO_PKG_VERSION"),
                        template.description()
                    );
                } else {
                    println!("  - {}", template.name());
                }
            }
            print_template_scope_entries(&workspace, TemplateScope::Workspace, verbose)?;
            print_template_scope_entries(&workspace, TemplateScope::User, verbose)?;
            print_template_scope_entries(&workspace, TemplateScope::Team, verbose)?;
            Ok(0)
        }
        TemplateCommands::Search {
            query,
            workspace,
            json,
        } => {
            if json {
                print!("{}", format_template_search_json(&workspace, &query)?);
            } else {
                print_template_search(&workspace, &query)?;
            }
            Ok(0)
        }
        TemplateCommands::Author {
            name,
            workspace,
            env,
            prompt,
            open,
            json,
        } => template_author_command(&name, &workspace, &env, prompt, open, json),
        TemplateCommands::Show {
            template,
            workspace,
            json,
        } => {
            if json {
                print!("{}", format_template_preview_json(&template, &workspace)?);
            } else {
                print_template_preview(&template, &workspace)?;
            }
            Ok(0)
        }
        TemplateCommands::Validate { path } => {
            let pack = load_template_pack(&path)?;
            validate_template_pack(&pack)?;
            println!("template: ok");
            println!("name: {}", pack.name);
            println!("agent: {}", pack.agent_name);
            println!("version: {}", pack.version);
            println!("description: {}", pack.description);
            Ok(0)
        }
        TemplateCommands::Export {
            template,
            workspace,
            output,
        } => {
            export_template_pack(&template, &workspace, &output)?;
            Ok(0)
        }
        TemplateCommands::Install {
            path,
            workspace,
            scope,
            name,
        } => {
            install_template_pack(
                &path,
                &workspace,
                name,
                "Installed",
                TemplateScope::parse(&scope)?,
            )?;
            Ok(0)
        }
        TemplateCommands::Import {
            path,
            workspace,
            scope,
            name,
        } => {
            install_template_pack(
                &path,
                &workspace,
                name,
                "Imported",
                TemplateScope::parse(&scope)?,
            )?;
            Ok(0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TemplateScope {
    Workspace,
    User,
    Team,
}

impl TemplateScope {
    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match value.to_ascii_lowercase().as_str() {
            "workspace" | "local" | "installed" => Ok(Self::Workspace),
            "user" | "personal" => Ok(Self::User),
            "team" | "shared" => Ok(Self::Team),
            other => Err(format!(
                "Unknown template scope \"{other}\". Use workspace, user, or team."
            )
            .into()),
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Workspace => "Installed templates",
            Self::User => "User templates",
            Self::Team => "Team templates",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Self::Workspace => "installed",
            Self::User => "user",
            Self::Team => "team",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::User => "user",
            Self::Team => "team",
        }
    }

    fn dir(self, workspace: &Path) -> PathBuf {
        let root = workspace.join(".agentic-harness/templates");
        match self {
            Self::Workspace => root,
            Self::User => root.join("user"),
            Self::Team => root.join("team"),
        }
    }
}

fn print_template_scope_entries(
    workspace: &Path,
    scope: TemplateScope,
    verbose: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n{}:", scope.heading());
    let entries = template_scope_entries(workspace, scope)?;
    if entries.is_empty() {
        println!("  none");
        return Ok(());
    }
    for (name, path) in entries {
        if verbose {
            match load_template_pack(&path).and_then(|pack| {
                validate_template_pack(&pack)?;
                Ok(pack)
            }) {
                Ok(pack) => println!(
                    "  - {name} (agent {}, version {}, {})",
                    pack.agent_name, pack.version, pack.description
                ),
                Err(err) => println!("  - {name} (invalid: {err})"),
            }
        } else {
            println!("  - {name}");
        }
    }
    Ok(())
}

fn format_template_list_json(workspace: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let templates = dashboard_template_json_entries(workspace)?;
    let count_source = |source: &str| {
        templates
            .iter()
            .filter(|template| {
                template.get("source").and_then(|value| value.as_str()) == Some(source)
            })
            .count()
    };
    let value = serde_json::json!({
        "workspace": workspace_display.display().to_string(),
        "counts": {
            "builtIn": count_source("built-in"),
            "installed": count_source("installed"),
            "user": count_source("user"),
            "team": count_source("team"),
        },
        "templates": templates,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn template_search_results(
    workspace: &Path,
    query: &str,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let query = query.trim().to_ascii_lowercase();
    let entries = dashboard_template_json_entries(workspace)?;
    if query.is_empty() {
        return Ok(entries);
    }
    Ok(entries
        .into_iter()
        .filter(|template| template_matches_query(template, &query))
        .collect())
}

fn template_matches_query(template: &serde_json::Value, query: &str) -> bool {
    ["name", "agent", "version", "description", "source"]
        .into_iter()
        .filter_map(|field| template.get(field).and_then(|value| value.as_str()))
        .any(|value| value.to_ascii_lowercase().contains(query))
}

fn format_template_search_json(
    workspace: &Path,
    query: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let results = template_search_results(workspace, query)?;
    let value = serde_json::json!({
        "workspace": workspace_display.display().to_string(),
        "query": query,
        "count": results.len(),
        "results": results,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn print_template_search(workspace: &Path, query: &str) -> Result<(), Box<dyn std::error::Error>> {
    let results = template_search_results(workspace, query)?;
    println!("Template search: {query}");
    if results.is_empty() {
        println!("  none");
        return Ok(());
    }
    for template in results {
        let name = template
            .get("name")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let source = template
            .get("source")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let agent = template
            .get("agent")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let version = template
            .get("version")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let description = template
            .get("description")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        println!("  - {name} ({source}, agent {agent}, version {version}) - {description}");
    }
    Ok(())
}

fn template_scope_entries(
    workspace: &Path,
    scope: TemplateScope,
) -> Result<Vec<(String, PathBuf)>, Box<dyn std::error::Error>> {
    let dir = scope.dir(workspace);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = fs::read_dir(dir)?
        .flatten()
        .filter(|entry| entry.path().join("agentic-template.toml").exists())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            Some((name, entry.path()))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(entries)
}

fn template_author_command(
    name: &str,
    workspace: &Path,
    env: &str,
    prompt: Option<String>,
    open: bool,
    json: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    validate_package_name(name)?;
    let environment = LlmAuthoringEnvironment::parse(env)?;
    if !llm_authoring_installed(workspace) {
        return Err(format!(
            "LLM authoring context is not installed in {}. Run `agentic-harness setup llm --workspace {} --env {}` first.",
            workspace.display(),
            workspace.display(),
            environment.slug()
        )
        .into());
    }
    let request = match prompt {
        Some(prompt) if !prompt.trim().is_empty() => prompt,
        _ if io::stdin().is_terminal() && io::stdout().is_terminal() => {
            prompt_required("Template goal")?
        }
        _ => format!("Create a reusable Agentic Harness template named {name}."),
    };
    let brief_dir = workspace.join(".agentic-harness/template-briefs");
    fs::create_dir_all(&brief_dir)?;
    let brief_path = brief_dir.join(format!("{name}.md"));
    let cli_path = current_cli_path();
    fs::write(
        &brief_path,
        render_template_authoring_brief(name, environment, &request, &cli_path),
    )?;
    let validation_commands = template_author_validation_commands(name);
    let open_command = llm_authoring_open_command(environment, &brief_path);
    if !json {
        println!("template brief: {}", brief_path.display());
        println!("environment: {}", environment.slug());
        println!("open with: {open_command}");
        println!("next:");
        for command in &validation_commands {
            println!("  {command}");
        }
    }
    let mut post_open_validation = None;
    if open {
        ensure_llm_tool_ready_for_open(environment)?;
        open_llm_authoring_environment(
            environment,
            workspace,
            &brief_path,
            if json {
                LlmInvocationEcho::Stderr
            } else {
                LlmInvocationEcho::StdoutStderr
            },
            !json,
        )?;
        let validation = validate_template_after_author_open(workspace, name)?;
        if !json {
            println!("post-open validation: ok");
            println!("template path: {}", validation.template_path.display());
            println!("next: {}", validation.next_command);
        }
        post_open_validation = Some(validation);
    }
    if json {
        print!(
            "{}",
            format_template_author_json(&TemplateAuthorJsonReport {
                workspace,
                name,
                environment,
                prompt: &request,
                brief_path: &brief_path,
                open_command: &open_command,
                validation_commands: &validation_commands,
                opened: open,
                post_open_validation: post_open_validation.as_ref(),
            })?
        );
    }
    Ok(0)
}

#[derive(Debug)]
struct TemplateAuthorPostOpenValidation {
    template_path: PathBuf,
    next_command: String,
}

fn validate_template_after_author_open(
    workspace: &Path,
    name: &str,
) -> Result<TemplateAuthorPostOpenValidation, Box<dyn std::error::Error>> {
    let path = workspace.join(name);
    let pack = load_template_pack(&path).map_err(|err| {
        format!(
            "Generated template pack was not found or could not be loaded at {}: {err}",
            path.display()
        )
    })?;
    validate_template_pack(&pack).map_err(|err| {
        format!(
            "Generated template pack at {} failed validation: {err}",
            path.display()
        )
    })?;
    let next_command = format!("agentic-harness template install ./{}", pack.name);
    Ok(TemplateAuthorPostOpenValidation {
        template_path: path,
        next_command,
    })
}

fn ensure_llm_tool_ready_for_open(
    environment: LlmAuthoringEnvironment,
) -> Result<(), Box<dyn std::error::Error>> {
    let command = llm_authoring_command_name(environment);
    let found = command_available(command);
    let (ready, detail) = llm_tool_readiness(environment, command, found);
    if ready {
        return Ok(());
    }
    Err(format!(
        "{} is not ready for template authoring: {detail}. Run `agentic-harness doctor --json` or authenticate `{command}` before using --open.",
        environment.label()
    )
    .into())
}

fn template_author_validation_commands(name: &str) -> Vec<String> {
    vec![
        format!("agentic-harness template validate ./{name}"),
        format!("agentic-harness template preview ./{name} --json"),
        format!("agentic-harness template install ./{name}"),
        format!("agentic-harness new ./{name}-agent --template {name}"),
        format!("agentic-harness doctor --workspace ./{name}-agent"),
    ]
}

struct TemplateAuthorJsonReport<'a> {
    workspace: &'a Path,
    name: &'a str,
    environment: LlmAuthoringEnvironment,
    prompt: &'a str,
    brief_path: &'a Path,
    open_command: &'a str,
    validation_commands: &'a [String],
    opened: bool,
    post_open_validation: Option<&'a TemplateAuthorPostOpenValidation>,
}

fn format_template_author_json(
    report: &TemplateAuthorJsonReport<'_>,
) -> Result<String, Box<dyn std::error::Error>> {
    let workspace_display = report
        .workspace
        .canonicalize()
        .unwrap_or_else(|_| report.workspace.to_path_buf());
    let value = serde_json::json!({
        "ok": true,
        "workspace": workspace_display.display().to_string(),
        "template": report.name,
        "environment": report.environment.slug(),
        "prompt": report.prompt,
        "briefPath": report.brief_path.display().to_string(),
        "openCommand": report.open_command,
        "opened": report.opened,
        "postOpenValidation": report.post_open_validation.map(|validation| {
            serde_json::json!({
                "ok": true,
                "templatePath": validation.template_path.display().to_string(),
                "nextCommand": validation.next_command,
            })
        }),
        "validationCommands": report.validation_commands,
        "nextCommands": report.validation_commands,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn render_template_authoring_brief(
    name: &str,
    environment: LlmAuthoringEnvironment,
    request: &str,
    cli_path: &Path,
) -> String {
    format!(
        r#"# Agentic Harness Template Brief: {name}

Environment: {}

Use `.agents/skills/agentic-harness-template/SKILL.md` before creating files.
Create a reusable Agentic Harness template pack, not a one-off SDK example.
Do not edit the Agentic Harness SDK or CLI crates while authoring this pack.

## User Request

{request}

## Output Shape

Create `./{name}` with this structure:

```text
{name}/
  agentic-template.toml
  files/
    Cargo.toml.hbs
    src/main.rs.hbs
    AGENTS.md.hbs
    .agentic-harness/roles/<role>.md
    .agents/skills/<skill>/SKILL.md
  examples/
    payload.json
```

The manifest must include:

```toml
name = "{name}"
agent = "<agent-name>"
version = "0.1.0"
description = "<what this template builds>"
sample_payload = "{{\"prompt\":\"Inspect this repository\"}}"
```

Use only these placeholders in `files/**/*.hbs`:

- `{{{{package_name}}}}`
- `{{{{template_name}}}}`
- `{{{{agent_name}}}}`
- `{{{{crate_path}}}}`

## Acceptance Commands

Run these before calling the template done:

```bash
agentic-harness template validate ./{name}
agentic-harness template preview ./{name} --json
agentic-harness template install ./{name}
agentic-harness new ./{name}-agent --template {name}
agentic-harness doctor --workspace ./{name}-agent
```

If `agentic-harness` is not on PATH, use this local CLI path for the same
commands:

```bash
"{}" template validate ./{name}
"{}" template preview ./{name} --json
"{}" template install ./{name}
"{}" new ./{name}-agent --template {name}
"{}" doctor --workspace ./{name}-agent
```

Keep credentials and provider secrets out of template files. Put sandbox or
model setup in explicit environment configuration and document required env vars.
"#,
        environment.label(),
        cli_path.display(),
        cli_path.display(),
        cli_path.display(),
        cli_path.display(),
        cli_path.display()
    )
}

fn current_cli_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("agentic-harness"))
}

fn llm_authoring_open_command(environment: LlmAuthoringEnvironment, brief_path: &Path) -> String {
    match environment {
        LlmAuthoringEnvironment::Codex => format!(
            "codex exec --sandbox workspace-write --skip-git-repo-check - < {}",
            brief_path.display()
        ),
        LlmAuthoringEnvironment::ClaudeCode => {
            "claude -p --permission-mode acceptEdits <brief>".to_string()
        }
        LlmAuthoringEnvironment::Cursor => {
            "cursor agent --print --trust --force <brief>".to_string()
        }
        LlmAuthoringEnvironment::WindServer => format!("wind-server {}", brief_path.display()),
    }
}

fn llm_authoring_command_name(environment: LlmAuthoringEnvironment) -> &'static str {
    match environment {
        LlmAuthoringEnvironment::ClaudeCode => "claude",
        LlmAuthoringEnvironment::Codex => "codex",
        LlmAuthoringEnvironment::Cursor => "cursor",
        LlmAuthoringEnvironment::WindServer => "wind-server",
    }
}

fn open_llm_authoring_environment(
    environment: LlmAuthoringEnvironment,
    workspace: &Path,
    brief_path: &Path,
    echo: LlmInvocationEcho,
    announce: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let invocation = llm_authoring_invocation(environment, brief_path)?;
    let command = invocation.command.clone();
    let manual_command = llm_authoring_open_command(environment, brief_path);
    let output = run_llm_invocation_with_echo(workspace, &invocation, echo).map_err(|err| {
        format!("Could not launch `{command}` for template authoring: {err}. Run `{manual_command}` manually.")
    })?;
    if !output.status.success() {
        return Err(format!(
            "`{command}` exited with {} while opening {}",
            output
                .status
                .code()
                .map(|code| format!("exit {code}"))
                .unwrap_or_else(|| "terminated".to_string()),
            brief_path.display()
        )
        .into());
    }
    if announce {
        println!("opened with: {command} {}", brief_path.display());
    }
    Ok(())
}

fn llm_authoring_invocation(
    environment: LlmAuthoringEnvironment,
    brief_path: &Path,
) -> Result<LlmCodingInvocation, Box<dyn std::error::Error>> {
    llm_prompt_invocation(environment, &fs::read_to_string(brief_path)?, brief_path)
}

fn install_template_pack(
    path: &Path,
    workspace: &Path,
    name: Option<String>,
    verb: &str,
    scope: TemplateScope,
) -> Result<(), Box<dyn std::error::Error>> {
    let pack = load_template_pack(path)?;
    validate_template_pack(&pack)?;
    let installed_name = name.unwrap_or_else(|| pack.name.clone());
    validate_package_name(&installed_name)?;
    let destination = scope.dir(workspace).join(&installed_name);
    if destination.exists() {
        return Err(format!(
            "Template {installed_name:?} already exists at {}",
            destination.display()
        )
        .into());
    }
    copy_dir_all(&pack.root, &destination)?;
    println!(
        "[agentic-harness] {verb} {} template {installed_name} at {}",
        scope.source(),
        destination.display()
    );
    Ok(())
}

fn export_template_pack(
    template: &str,
    workspace: &Path,
    output: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let (pack_path, source) = resolve_template_pack_from(workspace, template)?;
    let pack = load_template_pack(&pack_path)?;
    validate_template_pack(&pack)?;
    if output.exists() {
        if !output.is_dir() {
            return Err(
                format!("{} already exists and is not a directory", output.display()).into(),
            );
        }
        if output.read_dir()?.next().is_some() {
            return Err(format!("{} already exists and is not empty", output.display()).into());
        }
    }
    copy_dir_all(&pack.root, output)?;
    println!(
        "[agentic-harness] Exported template {} from {source} to {}",
        pack.name,
        output.display()
    );
    Ok(())
}

fn setup_command(command: SetupCommands) -> Result<u8, Box<dyn std::error::Error>> {
    match command {
        SetupCommands::Llm {
            workspace,
            env,
            print,
        } => setup_llm_command(&workspace, &env, print),
        SetupCommands::Sandbox {
            workspace,
            target,
            endpoint,
            print,
        } => setup_sandbox_command(&workspace, &target, endpoint.as_deref(), print),
        SetupCommands::Hosting { workspace, addr } => setup_hosting_command(&workspace, &addr),
    }
}

fn setup_llm_command(
    workspace: &Path,
    env: &str,
    print: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let environment = LlmAuthoringEnvironment::parse(env)?;
    let instructions = llm_authoring_instructions(environment);
    if print {
        print!("{instructions}");
        return Ok(0);
    }

    fs::create_dir_all(workspace.join(".agentic-harness"))?;
    fs::create_dir_all(workspace.join(".agents/skills/agentic-harness-template"))?;
    fs::write(
        workspace.join(".agentic-harness/llm-authoring.toml"),
        format!(
            "environment = \"{}\"\nskill = \".agents/skills/agentic-harness-template/SKILL.md\"\n",
            environment.slug()
        ),
    )?;
    fs::write(
        workspace.join(".agents/skills/agentic-harness-template/SKILL.md"),
        instructions,
    )?;
    install_template_authoring_examples(workspace)?;
    install_environment_hint(workspace, environment)?;
    append_agentic_harness_authoring_section(workspace)?;
    println!(
        "[agentic-harness] Installed {} template-authoring context in {}",
        environment.label(),
        workspace.display()
    );
    Ok(0)
}

fn setup_sandbox_command(
    workspace: &Path,
    target: &str,
    endpoint: Option<&str>,
    print: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let target = SandboxTarget::parse(target)?;
    let instructions = sandbox_setup_instructions(target);
    if print {
        print!("{instructions}");
        return Ok(0);
    }

    fs::create_dir_all(workspace.join(".agentic-harness"))?;
    fs::write(workspace.join(".agentic-harness/sandbox.toml"), {
        let mut config = format!(
            "target = \"{}\"\ncwd = \".\"\nsmoke_command = \"pwd\"\n",
            target.slug()
        );
        if let Some(endpoint) = endpoint {
            config.push_str(&format!(
                "endpoint = \"{}\"\n",
                escape_template_manifest_string(endpoint)
            ));
        }
        config
    })?;
    if target != SandboxTarget::Local {
        let connector_dir = workspace.join(".agentic-harness/connectors");
        fs::create_dir_all(&connector_dir)?;
        fs::write(
            connector_dir.join(format!("{}-sandbox.md", target.slug())),
            instructions,
        )?;
    }
    if target == SandboxTarget::Local {
        let smoke = run_local_sandbox_smoke(workspace);
        if !smoke.ok {
            return Err(format!(
                "Sandbox smoke check failed for {}: {}",
                workspace.display(),
                smoke.detail
            )
            .into());
        }
        println!(
            "sandbox: {} ready - {}",
            target.label().to_lowercase(),
            smoke.detail
        );
    } else if let Some(endpoint) = endpoint {
        println!(
            "sandbox: {} configured - endpoint {}",
            target.label().to_lowercase(),
            endpoint
        );
    } else {
        println!(
            "sandbox: {} configured - add endpoint to .agentic-harness/sandbox.toml before running remote operations",
            target.label().to_lowercase()
        );
    }
    Ok(0)
}

fn setup_hosting_command(workspace: &Path, addr: &str) -> Result<u8, Box<dyn std::error::Error>> {
    validate_local_host_addr(addr)?;
    fs::create_dir_all(workspace.join(".agentic-harness"))?;
    fs::write(
        hosting_config_path(workspace),
        format!(
            "addr = \"{}\"\nmode = \"local\"\n",
            escape_template_manifest_string(addr)
        ),
    )?;
    let status = hosting_status(workspace);
    println!(
        "hosting: local ready - {}",
        status
            .base_url
            .unwrap_or_else(|| format!("http://{}", status.addr))
    );
    println!("start: {}", status.start_command);
    println!("status: {}", status.status_command);
    Ok(0)
}

fn hosting_command(command: HostingCommands) -> Result<u8, Box<dyn std::error::Error>> {
    match command {
        HostingCommands::Status { workspace, json } => {
            let status = hosting_status(&workspace);
            if json {
                print!("{}", format_hosting_status_json(&status)?);
            } else {
                print!("{}", format_hosting_status_report(&status));
            }
            Ok(if status.servable { 0 } else { 1 })
        }
        HostingCommands::Start {
            workspace,
            addr,
            dev,
            env_files,
        } => host_command(&workspace, addr.as_deref(), dev, &env_files),
    }
}

fn host_command(
    workspace: &Path,
    addr: Option<&str>,
    dev: bool,
    env_files: &[PathBuf],
) -> Result<u8, Box<dyn std::error::Error>> {
    let configured = load_hosting_config(workspace);
    let addr = addr.unwrap_or(&configured.addr);
    validate_local_host_addr(addr)?;
    if dev {
        let port = addr
            .rsplit_once(':')
            .and_then(|(_, port)| port.parse::<u16>().ok())
            .ok_or_else(|| format!("Could not parse port from address {addr:?}"))?;
        return run_dev_server(workspace, port, env_files);
    }
    run_cargo(
        workspace,
        [
            "--agentic-harness-serve".to_string(),
            "--addr".to_string(),
            addr.to_string(),
        ],
        env_files,
    )
}

#[derive(Debug, Subcommand)]
enum SandboxCommands {
    /// Show configured sandbox target and readiness.
    Status {
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable sandbox status JSON.
        #[arg(long)]
        json: bool,
    },
    /// Execute a shell command in the local sandbox workspace.
    Exec {
        /// Shell command to execute.
        command: String,
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable command result JSON.
        #[arg(long)]
        json: bool,
    },
    /// Read a file from the local sandbox workspace.
    Read {
        /// Workspace-relative file path.
        path: PathBuf,
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Write a file into the local sandbox workspace.
    Write {
        /// Workspace-relative file path.
        path: PathBuf,
        /// Content to write.
        #[arg(long)]
        content: String,
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// List a directory in the local sandbox workspace.
    Ls {
        /// Workspace-relative directory path.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Sync a host file or directory into the local sandbox workspace.
    Sync {
        /// Host file or directory to copy.
        source: PathBuf,
        /// Workspace-relative destination path inside the sandbox.
        destination: PathBuf,
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Remove a file or directory from the local sandbox workspace.
    Rm {
        /// Workspace-relative file or directory path.
        path: PathBuf,
        /// Remove directories recursively.
        #[arg(long)]
        recursive: bool,
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
    },
    /// Print local sandbox operation logs.
    Logs {
        /// Workspace containing .agentic-harness/sandbox.toml.
        #[arg(long, default_value = ".")]
        workspace: PathBuf,
        /// Print machine-readable sandbox logs JSON.
        #[arg(long)]
        json: bool,
    },
}

fn sandbox_command(command: SandboxCommands) -> Result<u8, Box<dyn std::error::Error>> {
    match command {
        SandboxCommands::Status { workspace, json } => {
            let config = load_sandbox_config(&workspace);
            if json {
                let (body, ok) = format_sandbox_status_json(&workspace, &config)?;
                print!("{body}");
                return Ok(if ok { 0 } else { 1 });
            }
            println!("target: {}", config.target);
            println!("cwd: {}", config.cwd);
            if let Some(endpoint) = &config.endpoint {
                println!("endpoint: {endpoint}");
            }
            if config.target == "local" || config.endpoint.is_some() {
                let smoke = run_sandbox_smoke(&workspace, &config);
                println!(
                    "smoke: {} - {}",
                    if smoke.ok { "ok" } else { "failed" },
                    smoke.detail
                );
                Ok(if smoke.ok { 0 } else { 1 })
            } else {
                println!("operations: connector required for remote target");
                Ok(0)
            }
        }
        SandboxCommands::Exec {
            command,
            workspace,
            json,
        } => {
            let config = load_sandbox_config(&workspace);
            if config.target != "local" {
                let output = remote_sandbox_env(&config)?.exec(&command, ShellOptions::new())?;
                if json {
                    print!(
                        "{}",
                        format_sandbox_exec_json(&workspace, &config, &command, &output)?
                    );
                } else {
                    io::stdout().write_all(output.stdout.as_bytes())?;
                    io::stderr().write_all(output.stderr.as_bytes())?;
                }
                let code = output.exit_code as u8;
                append_sandbox_log(&workspace, &format!("exec -> {code}"))?;
                return Ok(code);
            }
            let root = local_sandbox_root(&workspace)?;
            let output = Command::new("sh")
                .arg("-c")
                .arg(&command)
                .current_dir(root)
                .output()?;
            let shell_output = ShellOutput {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(1),
            };
            if json {
                print!(
                    "{}",
                    format_sandbox_exec_json(&workspace, &config, &command, &shell_output)?
                );
            } else {
                io::stdout().write_all(&output.stdout)?;
                io::stderr().write_all(&output.stderr)?;
            }
            let code = output.status.code().unwrap_or(1) as u8;
            append_sandbox_log(&workspace, &format!("exec -> {code}"))?;
            Ok(code)
        }
        SandboxCommands::Read { path, workspace } => {
            let config = load_sandbox_config(&workspace);
            if config.target != "local" {
                let path = sandbox_relative_path_text(&path)?;
                print!("{}", remote_sandbox_env(&config)?.read_file(&path)?);
                append_sandbox_log(&workspace, &format!("read {path}"))?;
                return Ok(0);
            }
            let root = local_sandbox_root(&workspace)?;
            let path = resolve_sandbox_path(&root, &path)?;
            print!("{}", fs::read_to_string(&path)?);
            append_sandbox_log(
                &workspace,
                &format!(
                    "read {}",
                    path.strip_prefix(&root).unwrap_or(&path).display()
                ),
            )?;
            Ok(0)
        }
        SandboxCommands::Write {
            path,
            content,
            workspace,
        } => {
            let config = load_sandbox_config(&workspace);
            if config.target != "local" {
                let path = sandbox_relative_path_text(&path)?;
                remote_sandbox_env(&config)?.write_file(&path, content.as_bytes())?;
                println!("wrote: {path}");
                append_sandbox_log(&workspace, &format!("write {path}"))?;
                return Ok(0);
            }
            let root = local_sandbox_root(&workspace)?;
            let path = resolve_sandbox_path(&root, &path)?;
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, content)?;
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            println!("wrote: {}", relative);
            append_sandbox_log(&workspace, &format!("write {relative}"))?;
            Ok(0)
        }
        SandboxCommands::Ls { path, workspace } => {
            let config = load_sandbox_config(&workspace);
            if config.target != "local" {
                let path = sandbox_relative_path_text(&path)?;
                let mut entries = remote_sandbox_env(&config)?.readdir(&path)?;
                entries.sort();
                for entry in entries {
                    println!("{entry}");
                }
                append_sandbox_log(&workspace, &format!("ls {path}"))?;
                return Ok(0);
            }
            let root = local_sandbox_root(&workspace)?;
            let path = resolve_sandbox_path(&root, &path)?;
            let mut entries = fs::read_dir(&path)?
                .map(|entry| {
                    entry.map(|entry| {
                        let suffix = entry
                            .file_type()
                            .ok()
                            .filter(|file_type| file_type.is_dir())
                            .map(|_| "/")
                            .unwrap_or("");
                        format!("{}{}", entry.file_name().to_string_lossy(), suffix)
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            entries.sort();
            for entry in entries {
                println!("{entry}");
            }
            append_sandbox_log(
                &workspace,
                &format!("ls {}", path.strip_prefix(&root).unwrap_or(&path).display()),
            )?;
            Ok(0)
        }
        SandboxCommands::Sync {
            source,
            destination,
            workspace,
        } => {
            let config = load_sandbox_config(&workspace);
            if config.target != "local" {
                if !source.exists() {
                    return Err(format!("{} does not exist", source.display()).into());
                }
                let env = remote_sandbox_env(&config)?;
                sync_remote_sandbox_path(env.as_ref(), &source, &destination)?;
                let relative = sandbox_relative_path_text(&destination)?;
                println!("synced: {relative}");
                append_sandbox_log(&workspace, &format!("sync {relative}"))?;
                return Ok(0);
            }
            let root = local_sandbox_root(&workspace)?;
            if !source.exists() {
                return Err(format!("{} does not exist", source.display()).into());
            }
            let destination_path = resolve_sandbox_path(&root, &destination)?;
            if source.is_dir() {
                copy_dir_all(&source, &destination_path)?;
            } else {
                if let Some(parent) = destination_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&source, &destination_path)?;
            }
            let relative = destination_path
                .strip_prefix(&root)
                .unwrap_or(&destination_path)
                .display()
                .to_string();
            println!("synced: {relative}");
            append_sandbox_log(&workspace, &format!("sync {relative}"))?;
            Ok(0)
        }
        SandboxCommands::Rm {
            path,
            recursive,
            workspace,
        } => {
            if path.as_os_str().is_empty() || path == Path::new(".") {
                return Err("refusing to remove the sandbox root".into());
            }
            let config = load_sandbox_config(&workspace);
            if config.target != "local" {
                let path = sandbox_relative_path_text(&path)?;
                remote_sandbox_env(&config)?.rm(&path, recursive)?;
                println!("removed: {path}");
                append_sandbox_log(&workspace, &format!("rm {path}"))?;
                return Ok(0);
            }
            let root = local_sandbox_root(&workspace)?;
            let path = resolve_sandbox_path(&root, &path)?;
            if path.is_dir() {
                if !recursive {
                    return Err("use --recursive to remove a directory".into());
                }
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
            let relative = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            println!("removed: {relative}");
            append_sandbox_log(&workspace, &format!("rm {relative}"))?;
            Ok(0)
        }
        SandboxCommands::Logs { workspace, json } => {
            if json {
                print!("{}", format_sandbox_logs_json(&workspace)?);
                return Ok(0);
            }
            let log = sandbox_log_path(&workspace);
            if log.exists() {
                print!("{}", fs::read_to_string(log)?);
            }
            Ok(0)
        }
    }
}

fn sandbox_log_path(workspace: &Path) -> PathBuf {
    workspace.join(".agentic-harness/sandbox.log")
}

fn format_sandbox_status_json(
    workspace: &Path,
    config: &SandboxConfig,
) -> Result<(String, bool), Box<dyn std::error::Error>> {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let smoke = run_sandbox_smoke(workspace, config);
    let logs = recent_sandbox_logs(workspace, 5);
    let value = serde_json::json!({
        "workspace": workspace_display.display().to_string(),
        "configured": workspace.join(".agentic-harness/sandbox.toml").exists(),
        "target": config.target,
        "cwd": config.cwd,
        "endpoint": config.endpoint,
        "smoke": {
            "ok": smoke.ok,
            "detail": smoke.detail,
        },
        "capabilities": {
            "exec": true,
            "read": true,
            "write": true,
            "list": true,
            "sync": true,
            "cleanup": true,
            "logs": true,
            "httpEndpoint": config.endpoint.is_some(),
            "remoteConnectorRequired": config.target != "local" && config.endpoint.is_none(),
        },
        "recentLogCount": logs.len(),
        "recentLogs": logs,
    });
    Ok((
        format!("{}\n", serde_json::to_string_pretty(&value)?),
        smoke.ok,
    ))
}

fn format_sandbox_logs_json(workspace: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let log_path = sandbox_log_path(workspace);
    let log_path_display = log_path
        .canonicalize()
        .unwrap_or_else(|_| log_path.to_path_buf());
    let entries = fs::read_to_string(&log_path)
        .map(|content| {
            content
                .lines()
                .enumerate()
                .map(|(index, message)| {
                    serde_json::json!({
                        "index": index + 1,
                        "message": message,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let value = serde_json::json!({
        "workspace": workspace_display.display().to_string(),
        "logPath": log_path_display.display().to_string(),
        "count": entries.len(),
        "entries": entries,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn format_sandbox_exec_json(
    workspace: &Path,
    config: &SandboxConfig,
    command: &str,
    output: &ShellOutput,
) -> Result<String, Box<dyn std::error::Error>> {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let value = serde_json::json!({
        "workspace": workspace_display.display().to_string(),
        "target": config.target,
        "cwd": config.cwd,
        "command": command,
        "success": output.exit_code == 0,
        "exitCode": output.exit_code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn append_sandbox_log(workspace: &Path, line: &str) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(workspace.join(".agentic-harness"))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(sandbox_log_path(workspace))?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn remote_sandbox_env(
    config: &SandboxConfig,
) -> Result<Box<dyn SessionEnv>, Box<dyn std::error::Error>> {
    let endpoint = config.endpoint.as_deref().ok_or_else(|| {
        format!(
            "Sandbox target {:?} needs endpoint = \"https://...\" in .agentic-harness/sandbox.toml or setup with --endpoint.",
            config.target
        )
    })?;
    if let Some(path) = endpoint.strip_prefix("file://") {
        return Ok(Box::new(FileMockSessionEnv::new(path, &config.cwd)));
    }
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return Err(format!(
            "Unsupported sandbox endpoint {endpoint:?}; use http://, https://, or file:// for local smoke tests."
        )
        .into());
    }
    Ok(Box::new(HttpSessionEnv::new(endpoint, &config.cwd)))
}

fn sandbox_relative_path_text(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("sandbox paths must be relative and stay inside the sandbox cwd".into());
    }
    let value = path.display().to_string();
    if value.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(value)
    }
}

fn sync_remote_sandbox_path(
    env: &dyn SessionEnv,
    source: &Path,
    destination: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let destination_text = sandbox_relative_path_text(destination)?;
    if source.is_dir() {
        env.mkdir(&destination_text)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            sync_remote_sandbox_path(env, &entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        env.write_file(&destination_text, &fs::read(source)?)?;
    }
    Ok(())
}

fn sync_coding_workspace_to_remote_sandbox(
    workspace: &Path,
    config: &SandboxConfig,
) -> Result<(), Box<dyn std::error::Error>> {
    let env = remote_sandbox_env(config)?;
    sync_coding_workspace_path(env.as_ref(), workspace, Path::new(""))?;
    append_sandbox_log(workspace, "sync .")?;
    Ok(())
}

fn sync_coding_workspace_path(
    env: &dyn SessionEnv,
    source: &Path,
    destination: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let destination_text = sandbox_relative_path_text(destination)?;
    if source.is_dir() {
        env.mkdir(&destination_text)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            if should_skip_coding_sync_entry(&entry.path()) {
                continue;
            }
            sync_coding_workspace_path(env, &entry.path(), &destination.join(entry.file_name()))?;
        }
    } else {
        env.write_file(&destination_text, &fs::read(source)?)?;
    }
    Ok(())
}

fn should_skip_coding_sync_entry(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git" | "target" | "node_modules" | ".DS_Store" | "sandbox.log"
    ) || path.ends_with(".agentic-harness/runs")
}

#[derive(Debug, Clone)]
struct FileMockSessionEnv {
    log: PathBuf,
    cwd: PathBuf,
}

impl FileMockSessionEnv {
    fn new(log: impl AsRef<Path>, cwd: impl AsRef<Path>) -> Self {
        Self {
            log: log.as_ref().to_path_buf(),
            cwd: cwd.as_ref().to_path_buf(),
        }
    }

    fn record(&self, body: serde_json::Value) -> Result<(), AgenticHarnessError> {
        if let Some(parent) = self.log.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log)?;
        writeln!(file, "{body}")?;
        Ok(())
    }
}

impl SessionEnv for FileMockSessionEnv {
    fn exec(
        &self,
        command: &str,
        options: ShellOptions,
    ) -> Result<ShellOutput, AgenticHarnessError> {
        let cwd = options.cwd.as_ref().unwrap_or(&self.cwd);
        self.record(serde_json::json!({
            "op": "exec",
            "command": command,
            "cwd": cwd.to_string_lossy(),
            "env": options.env
        }))?;
        Ok(ShellOutput {
            stdout: format!("remote:{command}"),
            stderr: String::new(),
            exit_code: 0,
        })
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> {
        self.record(serde_json::json!({ "op": "read", "path": path }))?;
        Ok(format!("remote-file:{path}"))
    }

    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> {
        self.record(serde_json::json!({
            "op": "write",
            "path": path,
            "content": String::from_utf8_lossy(content)
        }))
    }

    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> {
        self.record(serde_json::json!({ "op": "stat", "path": path }))?;
        Ok(FileStat {
            is_file: true,
            is_directory: false,
            is_symbolic_link: false,
            size: 12,
            modified_unix_ms: Some(42),
        })
    }

    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> {
        self.record(serde_json::json!({ "op": "readdir", "path": path }))?;
        Ok(vec!["remote.txt".to_string(), "src".to_string()])
    }

    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> {
        self.record(serde_json::json!({ "op": "exists", "path": path }))?;
        Ok(true)
    }

    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> {
        self.record(serde_json::json!({ "op": "mkdir", "path": path }))
    }

    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> {
        self.record(serde_json::json!({
            "op": "rm",
            "path": path,
            "recursive": recursive
        }))
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

#[derive(Debug)]
struct HostingConfig {
    addr: String,
}

#[derive(Debug)]
struct HostingStatus {
    workspace: PathBuf,
    configured: bool,
    servable: bool,
    addr: String,
    base_url: Option<String>,
    health_url: Option<String>,
    agents_url: Option<String>,
    serve_command: String,
    dev_command: String,
    start_command: String,
    status_command: String,
    detail: String,
}

fn hosting_config_path(workspace: &Path) -> PathBuf {
    workspace.join(".agentic-harness/hosting.toml")
}

fn load_hosting_config(workspace: &Path) -> HostingConfig {
    let content = fs::read_to_string(hosting_config_path(workspace)).unwrap_or_default();
    let addr =
        parse_manifest_string(&content, "addr").unwrap_or_else(|_| "127.0.0.1:3583".to_string());
    HostingConfig { addr }
}

fn hosting_status(workspace: &Path) -> HostingStatus {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let config = load_hosting_config(workspace);
    let configured = hosting_config_path(workspace).exists();
    let cargo_toml = workspace.join("Cargo.toml");
    let main_rs = workspace.join("src/main.rs");
    let servable = cargo_toml.exists() && main_rs.exists();
    let base_url = hosting_base_url(&config.addr).ok();
    let health_url = base_url.as_ref().map(|url| format!("{url}/health"));
    let agents_url = base_url.as_ref().map(|url| format!("{url}/agents"));
    let workspace_arg = workspace.display();
    let serve_command = format!(
        "agentic-harness serve --workspace {workspace_arg} --addr {}",
        quote_cli_arg(&config.addr)
    );
    let dev_command = format!(
        "agentic-harness dev --workspace {workspace_arg} --port {}",
        hosting_port(&config.addr)
            .map(|port| port.to_string())
            .unwrap_or_else(|| "3583".to_string())
    );
    let start_command = format!("agentic-harness host --workspace {workspace_arg}");
    let status_command = format!("agentic-harness hosting status --workspace {workspace_arg}");
    let detail = if servable {
        "native app can be served locally".to_string()
    } else if !cargo_toml.exists() {
        format!("No Cargo.toml found at {}", cargo_toml.display())
    } else {
        format!("No src/main.rs found at {}", main_rs.display())
    };
    HostingStatus {
        workspace: workspace_display,
        configured,
        servable,
        addr: config.addr,
        base_url,
        health_url,
        agents_url,
        serve_command,
        dev_command,
        start_command,
        status_command,
        detail,
    }
}

fn validate_local_host_addr(addr: &str) -> Result<(), Box<dyn std::error::Error>> {
    if addr.trim().is_empty() {
        return Err("hosting address cannot be empty".into());
    }
    if addr.contains("://") {
        return Err("hosting address must be host:port, not a URL".into());
    }
    let host = addr.rsplit_once(':').map(|(host, _)| host).unwrap_or("");
    if !matches!(host, "127.0.0.1" | "localhost" | "::1" | "[::1]") {
        return Err(
            "local hosting only accepts loopback addresses: 127.0.0.1, localhost, or ::1".into(),
        );
    }
    if hosting_port(addr).is_none() {
        return Err(format!("hosting address needs a valid port: {addr}").into());
    }
    Ok(())
}

fn hosting_port(addr: &str) -> Option<u16> {
    addr.rsplit_once(':')
        .and_then(|(_, port)| port.trim_end_matches(']').parse::<u16>().ok())
}

fn hosting_base_url(addr: &str) -> Result<String, Box<dyn std::error::Error>> {
    validate_local_host_addr(addr)?;
    Ok(format!("http://{addr}"))
}

fn format_hosting_status_report(status: &HostingStatus) -> String {
    let mut out = String::new();
    out.push_str("Agentic Harness Local Hosting\n");
    out.push_str(&format!("workspace: {}\n", status.workspace.display()));
    out.push_str(&format!("configured: {}\n", status.configured));
    out.push_str(&format!(
        "servable: {} - {}\n",
        status.servable, status.detail
    ));
    out.push_str(&format!("addr: {}\n", status.addr));
    if let Some(base_url) = &status.base_url {
        out.push_str(&format!("base URL: {base_url}\n"));
    }
    if let Some(health_url) = &status.health_url {
        out.push_str(&format!("health: {health_url}\n"));
    }
    if let Some(agents_url) = &status.agents_url {
        out.push_str(&format!("agents: {agents_url}\n"));
    }
    out.push_str("capabilities: serve, dev reload, agent manifest, JSON invoke, SSE events\n");
    out.push_str("commands:\n");
    out.push_str(&format!("  start: {}\n", status.start_command));
    out.push_str(&format!("  serve: {}\n", status.serve_command));
    out.push_str(&format!("  dev: {}\n", status.dev_command));
    out.push_str(&format!("  status: {}\n", status.status_command));
    out
}

fn format_hosting_status_json(
    status: &HostingStatus,
) -> Result<String, Box<dyn std::error::Error>> {
    let value = serde_json::json!({
        "workspace": status.workspace.display().to_string(),
        "configured": status.configured,
        "servable": status.servable,
        "detail": status.detail,
        "addr": status.addr,
        "baseUrl": status.base_url,
        "healthUrl": status.health_url,
        "agentsUrl": status.agents_url,
        "capabilities": {
            "serve": true,
            "devReload": true,
            "health": true,
            "agentsManifest": true,
            "jsonInvoke": true,
            "sse": true,
            "localOnly": true,
            "deployment": false,
        },
        "commands": {
            "start": status.start_command,
            "serve": status.serve_command,
            "dev": status.dev_command,
            "status": status.status_command,
        },
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn dashboard_hosting_json(workspace: &Path) -> serde_json::Value {
    let status = hosting_status(workspace);
    serde_json::json!({
        "configured": status.configured,
        "servable": status.servable,
        "detail": status.detail,
        "addr": status.addr,
        "baseUrl": status.base_url,
        "healthUrl": status.health_url,
        "agentsUrl": status.agents_url,
        "commands": {
            "start": status.start_command,
            "serve": status.serve_command,
            "dev": status.dev_command,
            "status": status.status_command,
        },
    })
}

#[derive(Debug)]
struct SandboxConfig {
    target: String,
    cwd: String,
    endpoint: Option<String>,
}

fn load_sandbox_config(workspace: &Path) -> SandboxConfig {
    let config = workspace.join(".agentic-harness/sandbox.toml");
    let content = fs::read_to_string(config).unwrap_or_default();
    let target = parse_manifest_string(&content, "target").unwrap_or_else(|_| "local".to_string());
    let cwd = parse_manifest_string(&content, "cwd").unwrap_or_else(|_| ".".to_string());
    let endpoint = parse_manifest_string(&content, "endpoint").ok();
    SandboxConfig {
        target,
        cwd,
        endpoint,
    }
}

fn local_sandbox_root(workspace: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config = load_sandbox_config(workspace);
    if config.target != "local" {
        return Err(format!(
            "Sandbox target {:?} needs a SessionEnv connector; local file and shell operations only run with target = \"local\".",
            config.target
        )
        .into());
    }
    let workspace = workspace.canonicalize()?;
    let cwd = Path::new(&config.cwd);
    if cwd.is_absolute()
        || cwd
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("sandbox cwd must stay inside the workspace".into());
    }
    let root = workspace.join(cwd);
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn resolve_sandbox_path(root: &Path, path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("sandbox paths must be relative and stay inside the sandbox cwd".into());
    }
    Ok(root.join(path))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LlmAuthoringEnvironment {
    ClaudeCode,
    Codex,
    Cursor,
    WindServer,
}

impl LlmAuthoringEnvironment {
    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match value.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::detect().unwrap_or(Self::Codex)),
            "claude" | "claude-code" | "claude_code" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            "cursor" => Ok(Self::Cursor),
            "wind" | "wind-server" | "wind_server" => Ok(Self::WindServer),
            other => Err(format!(
                "Unknown LLM environment \"{other}\". Use claude-code, codex, cursor, wind-server, or auto."
            )
            .into()),
        }
    }

    fn detect() -> Option<Self> {
        [
            Self::ClaudeCode,
            Self::Codex,
            Self::Cursor,
            Self::WindServer,
        ]
        .into_iter()
        .find(|environment| command_available(llm_authoring_command_name(*environment)))
    }

    fn slug(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::Cursor => "cursor",
            Self::WindServer => "wind-server",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Cursor => "Cursor",
            Self::WindServer => "Wind Server",
        }
    }
}

fn command_available(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| file_is_executable(&dir.join(command)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SandboxTarget {
    Local,
    Vercel,
    Daytona,
    E2b,
    Custom,
}

impl SandboxTarget {
    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match value.to_ascii_lowercase().as_str() {
            "local" => Ok(Self::Local),
            "vercel" | "vercel-sandbox" => Ok(Self::Vercel),
            "daytona" => Ok(Self::Daytona),
            "e2b" => Ok(Self::E2b),
            "custom" | "http" | "remote" => Ok(Self::Custom),
            other => Err(format!(
                "Unknown sandbox target \"{other}\". Use local, vercel, daytona, e2b, or custom."
            )
            .into()),
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Vercel => "vercel",
            Self::Daytona => "daytona",
            Self::E2b => "e2b",
            Self::Custom => "custom",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Local => "Local checkout",
            Self::Vercel => "Vercel Sandbox",
            Self::Daytona => "Daytona",
            Self::E2b => "E2B",
            Self::Custom => "Custom remote sandbox",
        }
    }
}

struct SandboxSmoke {
    ok: bool,
    detail: String,
}

fn run_local_sandbox_smoke(workspace: &Path) -> SandboxSmoke {
    if !workspace.is_dir() {
        return SandboxSmoke {
            ok: false,
            detail: format!("workspace directory not found at {}", workspace.display()),
        };
    }
    if fs::read_dir(workspace).is_err() {
        return SandboxSmoke {
            ok: false,
            detail: "could not list workspace files".to_string(),
        };
    }
    match Command::new("pwd").current_dir(workspace).output() {
        Ok(output) if output.status.success() => SandboxSmoke {
            ok: true,
            detail: "local checkout smoke ok".to_string(),
        },
        Ok(output) => SandboxSmoke {
            ok: false,
            detail: format!("pwd exited with {}", output.status),
        },
        Err(err) => SandboxSmoke {
            ok: false,
            detail: format!("could not run pwd: {err}"),
        },
    }
}

fn run_sandbox_smoke(workspace: &Path, config: &SandboxConfig) -> SandboxSmoke {
    if config.target == "local" {
        return run_local_sandbox_smoke(workspace);
    }
    if config.endpoint.is_none() {
        return SandboxSmoke {
            ok: true,
            detail: "connector required for remote target".to_string(),
        };
    }
    let env = match remote_sandbox_env(config) {
        Ok(env) => env,
        Err(err) => {
            return SandboxSmoke {
                ok: false,
                detail: format!("remote endpoint smoke failed: {err}"),
            };
        }
    };
    let exec = match env.exec("pwd", ShellOptions::new()) {
        Ok(output) if output.exit_code == 0 => output,
        Ok(output) => {
            return SandboxSmoke {
                ok: false,
                detail: format!("remote pwd exited with {}", output.exit_code),
            };
        }
        Err(err) => {
            return SandboxSmoke {
                ok: false,
                detail: format!("remote pwd failed: {err}"),
            };
        }
    };
    let entries = match env.readdir(".") {
        Ok(entries) => entries,
        Err(err) => {
            return SandboxSmoke {
                ok: false,
                detail: format!("remote list failed: {err}"),
            };
        }
    };
    SandboxSmoke {
        ok: true,
        detail: format!(
            "remote endpoint smoke ok; pwd returned {} bytes and listed {} entries",
            exec.stdout.len(),
            entries.len()
        ),
    }
}

fn llm_authoring_instructions(environment: LlmAuthoringEnvironment) -> String {
    format!(
        r#"---
name: agentic-harness-template
description: Create reusable Agentic Harness template packs
---

# Agentic Harness Template Authoring For {}

Use this when the user wants to create or refine an Agentic Harness agent
template. Produce reusable template packs, not one-off SDK snippets.

Required template shape:

```text
template-name/
  agentic-template.toml
  files/
    Cargo.toml.hbs
    src/main.rs.hbs
    AGENTS.md.hbs
    .agentic-harness/roles/coder.md
    .agents/skills/coding/SKILL.md
  examples/
    payload.json
```

Required `agentic-template.toml` keys:

```toml
name = "template-name"
agent = "agent-name"
description = "What this agent does"
sample_payload = "{{\"prompt\":\"Inspect this repository\"}}"
```

Supported placeholders inside `files/**/*.hbs`:

- `{{{{package_name}}}}`
- `{{{{template_name}}}}`
- `{{{{agent_name}}}}`
- `{{{{crate_path}}}}`

Validation commands:

```bash
agentic-harness template validate ./template-name
agentic-harness template install ./template-name
agentic-harness new ./my-agent --template template-name
agentic-harness doctor --workspace ./my-agent
```

Installed reference examples:

- `.agentic-harness/template-examples/code-review`

Use the reference example when shaping new packs. It is intentionally small,
validates with `agentic-harness template validate`, and shows where to place
Rust entrypoints, roles, skills, and example payloads.

Keep provider credentials out of templates. Put provider setup in explicit env
or sandbox configuration, then run `agentic-harness doctor`.
"#,
        environment.label()
    )
}

fn install_template_authoring_examples(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let root = workspace.join(".agentic-harness/template-examples/code-review");
    fs::create_dir_all(root.join("files/src"))?;
    fs::create_dir_all(root.join("files/.agentic-harness/roles"))?;
    fs::create_dir_all(root.join("files/.agents/skills/code-review"))?;
    fs::create_dir_all(root.join("examples"))?;

    let sample_payload = serde_json::json!({
        "prompt": "Review the current repository changes",
        "focus": "correctness, tests, and maintainability"
    })
    .to_string();
    fs::write(
        root.join("agentic-template.toml"),
        format!(
            "name = \"code-review\"\nagent = \"code-review\"\nversion = \"0.1.0\"\ndescription = \"Repository code review coding agent\"\nsample_payload = \"{}\"\n",
            escape_template_manifest_string(&sample_payload)
        ),
    )?;
    fs::write(
        root.join("files/Cargo.toml.hbs"),
        template_init_cargo_toml_hbs(),
    )?;
    fs::write(
        root.join("files/src/main.rs.hbs"),
        code_review_template_main_rs_hbs(),
    )?;
    fs::write(
        root.join("files/AGENTS.md.hbs"),
        "# {{{{package_name}}}}\n\nThis repository was scaffolded from the `code-review` Agentic Harness template.\n\nUse the included code review role and skill before editing or reporting findings.\n",
    )?;
    fs::write(
        root.join("files/.agentic-harness/roles/reviewer.md"),
        "---\ndescription: Review repository changes for correctness, regressions, and missing tests\n---\nYou are a focused code reviewer. Lead with concrete findings, cite affected files, and call out test gaps.\n",
    )?;
    fs::write(
        root.join("files/.agents/skills/code-review/SKILL.md"),
        "---\nname: code-review\ndescription: Review repository changes and produce actionable findings\n---\n\nInspect the diff, read the surrounding code, run targeted checks when available, and return concise findings with file references.\n",
    )?;
    fs::write(
        root.join("examples/payload.json"),
        format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(
                &sample_payload
            )?)?
        ),
    )?;
    Ok(())
}

fn code_review_template_main_rs_hbs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct ReviewPayload {
    prompt: Option<String>,
    focus: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new()
        .with_workspace(".")
        .load_workspace_context()?
        .agent(AgentDefinition::webhook("{{agent_name}}", |ctx: AgentContext| {
            let payload: ReviewPayload = ctx.payload()?;
            Ok(json!({
                "id": ctx.id(),
                "agent": "{{agent_name}}",
                "template": "{{template_name}}",
                "prompt": payload.prompt.unwrap_or_else(|| "Review the repository changes".to_string()),
                "focus": payload.focus.unwrap_or_else(|| "correctness, tests, and maintainability".to_string()),
                "summary": "Read the diff, inspect related files, run focused checks, and report actionable findings."
            }))
        })))
}

fn main() {
    let code = match app().and_then(run_cli) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("[agentic-harness] {err}");
            1
        }
    };
    std::process::exit(code);
}
"#
}

fn install_environment_hint(
    workspace: &Path,
    environment: LlmAuthoringEnvironment,
) -> Result<(), Box<dyn std::error::Error>> {
    match environment {
        LlmAuthoringEnvironment::ClaudeCode => append_or_create(
            &workspace.join("CLAUDE.md"),
            "\n## Agentic Harness Template Authoring\n\nUse `.agents/skills/agentic-harness-template/SKILL.md` when creating reusable Agentic Harness templates.\n",
        ),
        LlmAuthoringEnvironment::Codex => append_or_create(
            &workspace.join("AGENTS.md"),
            "\n## Agentic Harness Template Authoring\n\nUse `.agents/skills/agentic-harness-template/SKILL.md` before creating reusable Agentic Harness templates.\n",
        ),
        LlmAuthoringEnvironment::Cursor => {
            let path = workspace.join(".cursor/rules/agentic-harness-template.mdc");
            fs::create_dir_all(path.parent().ok_or("missing cursor rule parent")?)?;
            fs::write(
                path,
                "Use .agents/skills/agentic-harness-template/SKILL.md to create reusable Agentic Harness templates.\n",
            )?;
            Ok(())
        }
        LlmAuthoringEnvironment::WindServer => {
            let path = workspace.join(".wind/agentic-harness-template.md");
            fs::create_dir_all(path.parent().ok_or("missing wind rule parent")?)?;
            fs::write(
                path,
                "Use .agents/skills/agentic-harness-template/SKILL.md to create reusable Agentic Harness templates.\n",
            )?;
            Ok(())
        }
    }
}

fn append_agentic_harness_authoring_section(
    workspace: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    append_or_create(
        &workspace.join("AGENTS.md"),
        "\n## Agentic Harness Template Authoring\n\nWhen the user asks for a new agent shape, create a reusable template pack with `agentic-template.toml`, templated Rust files, roles, skills, an example payload, and validation commands.\n",
    )
}

fn append_or_create(path: &Path, section: &str) -> Result<(), Box<dyn std::error::Error>> {
    let existing = fs::read_to_string(path).unwrap_or_default();
    if existing.contains("Agentic Harness Template Authoring") {
        return Ok(());
    }
    let mut next = existing;
    if !next.ends_with('\n') && !next.is_empty() {
        next.push('\n');
    }
    next.push_str(section);
    fs::write(path, next)?;
    Ok(())
}

fn sandbox_setup_instructions(target: SandboxTarget) -> String {
    match target {
        SandboxTarget::Local => {
            "# Local checkout sandbox\n\nAgentic Harness will run shell and file operations in the selected local workspace. No credentials are required.\n".to_string()
        }
        SandboxTarget::Vercel => native_vercel_connector_markdown().to_string(),
        SandboxTarget::Daytona => native_daytona_connector_markdown().to_string(),
        SandboxTarget::E2b => native_e2b_connector_markdown().to_string(),
        SandboxTarget::Custom => native_sandbox_category_markdown().replace("{{URL}}", "your provider documentation"),
    }
}

#[derive(Debug)]
struct DoctorReport {
    workspace: PathBuf,
    checks: Vec<DoctorCheck>,
}

#[derive(Debug)]
struct DoctorCheck {
    label: &'static str,
    ok: bool,
    required: bool,
    detail: String,
    fix: &'static str,
}

impl DoctorReport {
    fn required_ready(&self) -> bool {
        self.checks
            .iter()
            .filter(|check| check.required)
            .all(|check| check.ok)
    }

    fn has_optional_warnings(&self) -> bool {
        self.checks
            .iter()
            .filter(|check| !check.required)
            .any(|check| !check.ok)
    }
}

fn doctor_command(
    workspace: &Path,
    plain: bool,
    json: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let report = doctor_report(workspace);
    if json {
        print!("{}", format_doctor_json(&report)?);
    } else {
        print!("{}", format_doctor_report(&report, !plain));
    }
    Ok(if report.required_ready() { 0 } else { 1 })
}

fn dashboard_command(
    workspace: &Path,
    plain: bool,
    json: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let report = doctor_report(workspace);
    if json {
        print!("{}", format_dashboard_json(workspace, &report)?);
    } else {
        print!("{}", format_dashboard_report(workspace, &report, !plain)?);
    }
    Ok(if report.required_ready() { 0 } else { 1 })
}

fn result_command(workspace: &Path, json: bool) -> Result<u8, Box<dyn std::error::Error>> {
    let relative = if json {
        DEFAULT_CODING_SUMMARY_JSON_PATH
    } else {
        DEFAULT_CODING_SUMMARY_PATH
    };
    let path = workspace.join(relative);
    let body = fs::read_to_string(&path).map_err(|_| {
        format!(
            "No coding run summary found at {}. Run `agentic-harness code --workspace {} --prompt \"Describe the software change\"` first.",
            path.display(),
            workspace.display()
        )
    })?;
    print!("{body}");
    Ok(0)
}

#[derive(Debug)]
struct GuideStep {
    id: &'static str,
    title: &'static str,
    detail: String,
    command: String,
    status: &'static str,
}

fn guide_command(
    workspace: &Path,
    env: &str,
    template: &str,
    prompt: &str,
    json: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let environment = LlmAuthoringEnvironment::parse(env)?;
    validate_package_name(template)?;
    let steps = guide_steps(workspace, environment, template, prompt);
    if json {
        print!(
            "{}",
            format_guide_json(workspace, environment, template, &steps)?
        );
    } else {
        print!(
            "{}",
            format_guide_report(workspace, environment, template, &steps)
        );
    }
    Ok(0)
}

fn smoke_command(workspace: &Path, json: bool) -> Result<u8, Box<dyn std::error::Error>> {
    let report = doctor_report(workspace);
    let config = load_sandbox_config(workspace);
    let sandbox_smoke = run_sandbox_smoke(workspace, &config);
    let wizard_plain = wizard_dashboard(Path::new(".")).contains("Agentic Harness Wizard");
    let llm_tools = llm_tool_availability();
    let version = format!("agentic-harness {}", env!("CARGO_PKG_VERSION"));
    let ok = wizard_plain && report.required_ready() && sandbox_smoke.ok;
    let next_command = if ok {
        format!(
            "agentic-harness code --workspace {}",
            report.workspace.display()
        )
    } else {
        "agentic-harness doctor --json".to_string()
    };

    if json {
        let value = serde_json::json!({
            "ok": ok,
            "version": version,
            "workspace": report.workspace.display().to_string(),
            "nextCommand": next_command,
            "checks": {
                "wizardPlain": wizard_plain,
                "doctorRequiredReady": report.required_ready(),
                "doctorOptionalWarnings": report.has_optional_warnings(),
                "sandboxSmoke": {
                    "ok": sandbox_smoke.ok,
                    "detail": sandbox_smoke.detail,
                    "target": config.target,
                    "cwd": config.cwd,
                    "endpoint": config.endpoint,
                },
                "llmTools": {
                    "available": llm_tools.iter().any(|tool| tool.ready),
                    "tools": llm_tool_json_entries(&llm_tools),
                },
            },
            "doctor": {
                "checks": report.checks.iter().map(|check| {
                    serde_json::json!({
                        "label": check.label,
                        "ok": check.ok,
                        "required": check.required,
                        "detail": check.detail,
                        "fix": check.fix,
                    })
                }).collect::<Vec<_>>(),
            },
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Agentic Harness Smoke");
        println!("version: {version}");
        println!("wizard: {}", if wizard_plain { "ok" } else { "failed" });
        println!(
            "doctor: {}",
            if report.required_ready() {
                "ok"
            } else {
                "failed"
            }
        );
        println!(
            "sandbox: {} - {}",
            if sandbox_smoke.ok { "ok" } else { "failed" },
            sandbox_smoke.detail
        );
        println!("llm tools: {}", llm_tool_status());
        println!("next: {next_command}");
    }

    Ok(if ok { 0 } else { 1 })
}

#[derive(Debug)]
struct ReleasePackagingCheck {
    key: &'static str,
    label: &'static str,
    ok: bool,
    detail: String,
    fix: &'static str,
}

fn release_check_command(root: &Path, json: bool) -> Result<u8, Box<dyn std::error::Error>> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let checks = release_packaging_checks(&root);
    let ok = checks.iter().all(|check| check.ok);
    let next_commands = release_check_next_commands();
    if json {
        let checks_json = checks
            .iter()
            .map(|check| {
                (
                    check.key.to_string(),
                    serde_json::json!({
                        "label": check.label,
                        "ok": check.ok,
                        "detail": check.detail,
                        "fix": check.fix,
                    }),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let value = serde_json::json!({
            "ok": ok,
            "version": env!("CARGO_PKG_VERSION"),
            "root": root.display().to_string(),
            "checks": checks_json,
            "nextCommands": next_commands,
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Agentic Harness Release Check");
        println!("version: {}", env!("CARGO_PKG_VERSION"));
        println!("root: {}", root.display());
        for check in &checks {
            println!(
                "{}: {} - {}",
                check.label,
                if check.ok { "ok" } else { "missing" },
                check.detail
            );
        }
        println!("next:");
        for command in next_commands {
            println!("  {command}");
        }
    }
    Ok(if ok { 0 } else { 1 })
}

fn release_packaging_checks(root: &Path) -> Vec<ReleasePackagingCheck> {
    vec![
        release_install_script_check(root),
        release_formula_check(root),
        release_changelog_check(root),
        release_binary_package_check(root),
        release_smoke_doc_check(root),
    ]
}

fn release_check_next_commands() -> Vec<&'static str> {
    vec![
        "cargo fmt --all --check",
        "cargo test --workspace",
        "cargo clippy --workspace -- -D warnings",
        "agentic-harness package --output dist/packages --json",
        "agentic-harness release-check --json",
    ]
}

fn release_install_script_check(root: &Path) -> ReleasePackagingCheck {
    let path = root.join("scripts/install.sh");
    let mut problems = Vec::new();
    match fs::read_to_string(&path) {
        Ok(body) => {
            if !body.contains("cargo install --path") {
                problems.push("missing local cargo install path");
            }
            if !body.contains("agentic-harness") {
                problems.push("missing binary name");
            }
            if !body.contains("--version") {
                problems.push("missing post-install version check");
            }
        }
        Err(_) => problems.push("missing scripts/install.sh"),
    }
    if !path.exists() {
        problems.push("missing file");
    }
    if path.exists() && !file_is_executable(&path) {
        problems.push("not executable");
    }
    if path.exists() {
        match Command::new("bash").arg("-n").arg(&path).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => problems.push(if output.stderr.is_empty() {
                "bash syntax check failed"
            } else {
                "bash syntax check failed with stderr"
            }),
            Err(_) => problems.push("could not run bash syntax check"),
        }
    }
    release_check(
        "installScript",
        "install script",
        problems,
        "Fix scripts/install.sh and run bash -n scripts/install.sh.",
    )
}

fn release_formula_check(root: &Path) -> ReleasePackagingCheck {
    let path = root.join("Formula/agentic-harness.rb");
    let mut problems = Vec::new();
    match fs::read_to_string(&path) {
        Ok(body) => {
            for (needle, problem) in [
                (
                    "class AgenticHarness < Formula",
                    "missing formula class name",
                ),
                ("license \"Apache-2.0\"", "missing license"),
                ("\"cargo\"", "missing cargo install invocation"),
                ("\"install\"", "missing install command"),
                ("agentic-harness --version", "missing version smoke test"),
            ] {
                if !body.contains(needle) {
                    problems.push(problem);
                }
            }
        }
        Err(_) => problems.push("missing Formula/agentic-harness.rb"),
    }
    if path.exists() {
        match Command::new("ruby").arg("-c").arg(&path).output() {
            Ok(output) if output.status.success() => {}
            Ok(output) => problems.push(if output.stderr.is_empty() {
                "ruby syntax check failed"
            } else {
                "ruby syntax check failed with stderr"
            }),
            Err(_) => problems.push("could not run ruby syntax check"),
        }
    }
    release_check(
        "homebrewFormula",
        "homebrew formula",
        problems,
        "Fix Formula/agentic-harness.rb before publishing.",
    )
}

fn release_changelog_check(root: &Path) -> ReleasePackagingCheck {
    let path = root.join("CHANGELOG.md");
    let mut problems = Vec::new();
    match fs::read_to_string(&path) {
        Ok(body) => {
            if !body.contains(&format!("## {}", env!("CARGO_PKG_VERSION"))) {
                problems.push("missing current version section");
            }
            if !body.contains("Native Rust") {
                problems.push("missing native Rust release note");
            }
            if !body.contains("Release packaging") {
                problems.push("missing packaging release note");
            }
        }
        Err(_) => problems.push("missing CHANGELOG.md"),
    }
    release_check(
        "changelog",
        "changelog",
        problems,
        "Update CHANGELOG.md for the current version.",
    )
}

fn release_binary_package_check(root: &Path) -> ReleasePackagingCheck {
    let mut problems = Vec::new();
    for (path, needle, problem) in [
        (
            root.join("README.md"),
            "agentic-harness package --output",
            "README missing package command",
        ),
        (
            root.join("docs/release-smoke-test.md"),
            "agentic-harness package --output",
            "release smoke doc missing package command",
        ),
        (
            root.join("CHANGELOG.md"),
            "binary package",
            "changelog missing binary package note",
        ),
    ] {
        match fs::read_to_string(path) {
            Ok(body) if body.contains(needle) => {}
            Ok(_) => problems.push(problem),
            Err(_) => problems.push(problem),
        }
    }
    release_check(
        "binaryPackage",
        "binary package",
        problems,
        "Document and verify `agentic-harness package --output dist/packages --json` before publishing.",
    )
}

fn release_smoke_doc_check(root: &Path) -> ReleasePackagingCheck {
    let path = root.join("docs/release-smoke-test.md");
    let mut problems = Vec::new();
    match fs::read_to_string(&path) {
        Ok(body) => {
            for (needle, problem) in [
                ("agentic-harness --version", "missing version smoke command"),
                ("agentic-harness smoke --json", "missing CLI smoke command"),
                (
                    "agentic-harness release-check --json",
                    "missing release-check command",
                ),
                ("agentic-harness tui --plain", "missing TUI smoke command"),
                (
                    "agentic-harness inspect --workspace",
                    "missing result inspection command",
                ),
                ("cargo test --workspace", "missing workspace test command"),
                (
                    "cargo clippy --workspace -- -D warnings",
                    "missing clippy gate",
                ),
            ] {
                if !body.contains(needle) {
                    problems.push(problem);
                }
            }
        }
        Err(_) => problems.push("missing docs/release-smoke-test.md"),
    }
    release_check(
        "releaseSmokeDoc",
        "release smoke doc",
        problems,
        "Update docs/release-smoke-test.md with the clean-machine checks.",
    )
}

fn release_check(
    key: &'static str,
    label: &'static str,
    problems: Vec<&'static str>,
    fix: &'static str,
) -> ReleasePackagingCheck {
    ReleasePackagingCheck {
        key,
        label,
        ok: problems.is_empty(),
        detail: if problems.is_empty() {
            "ready".to_string()
        } else {
            problems.join(", ")
        },
        fix,
    }
}

#[derive(Debug)]
struct PackageArtifact {
    dir: PathBuf,
    binary: PathBuf,
    archive: PathBuf,
    manifest: PathBuf,
    checksums: PathBuf,
    artifact_name: String,
    binary_name: String,
    archive_name: String,
    version: &'static str,
    os: &'static str,
    arch: &'static str,
    size_bytes: u64,
    sha256: String,
    archive_size_bytes: u64,
    archive_sha256: String,
}

fn package_command(
    output: &Path,
    binary: Option<&Path>,
    json: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let artifact = package_binary(output, binary)?;
    if json {
        let value = serde_json::json!({
            "ok": true,
            "artifact": package_artifact_json(&artifact),
        });
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("Agentic Harness Package");
        println!("version: {}", artifact.version);
        println!("artifact: {}", artifact.dir.display());
        println!("binary: {}", artifact.binary.display());
        println!("archive: {}", artifact.archive.display());
        println!("manifest: {}", artifact.manifest.display());
        println!("checksums: {}", artifact.checksums.display());
        println!("sha256: {}", artifact.sha256);
        println!("archive sha256: {}", artifact.archive_sha256);
    }
    Ok(0)
}

fn package_binary(
    output: &Path,
    binary: Option<&Path>,
) -> Result<PackageArtifact, Box<dyn std::error::Error>> {
    let source = match binary {
        Some(path) => path.to_path_buf(),
        None => std::env::current_exe()?,
    };
    if !source.is_file() {
        return Err(format!("package binary not found at {}", source.display()).into());
    }
    let version = env!("CARGO_PKG_VERSION");
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let artifact_name = format!("agentic-harness-v{version}-{os}-{arch}");
    let dir = output.join(&artifact_name);
    fs::create_dir_all(&dir)?;
    let binary_name = format!("agentic-harness{}", std::env::consts::EXE_SUFFIX);
    let binary_path = dir.join(&binary_name);
    fs::copy(&source, &binary_path)?;
    make_executable(&binary_path)?;
    let bytes = fs::read(&binary_path)?;
    let sha256 = sha256_hex(&bytes);
    let size_bytes = bytes.len() as u64;
    let archive_name = format!("{artifact_name}.tar");
    let archive = output.join(&archive_name);
    let archive_manifest = format!(
        "{}\n",
        serde_json::to_string_pretty(&package_archive_manifest_json(
            version,
            os,
            arch,
            &artifact_name,
            &binary_name,
            size_bytes,
            &sha256,
        ))?
    )
    .into_bytes();
    write_tar_archive(
        &archive,
        &[
            TarEntry {
                name: format!("{artifact_name}/{binary_name}"),
                mode: 0o755,
                bytes,
            },
            TarEntry {
                name: format!("{artifact_name}/manifest.json"),
                mode: 0o644,
                bytes: archive_manifest,
            },
        ],
    )?;
    let archive_bytes = fs::read(&archive)?;
    let archive_sha256 = sha256_hex(&archive_bytes);
    let archive_size_bytes = archive_bytes.len() as u64;
    let manifest = dir.join("manifest.json");
    let checksums = dir.join("SHA256SUMS");
    let artifact = PackageArtifact {
        dir,
        binary: binary_path,
        archive,
        manifest,
        checksums,
        artifact_name,
        binary_name,
        archive_name,
        version,
        os,
        arch,
        size_bytes,
        sha256,
        archive_size_bytes,
        archive_sha256,
    };
    fs::write(
        &artifact.manifest,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&package_artifact_json(&artifact))?
        ),
    )?;
    fs::write(
        &artifact.checksums,
        format!(
            "{}  {}\n{}  ../{}\n",
            artifact.sha256, artifact.binary_name, artifact.archive_sha256, artifact.archive_name
        ),
    )?;
    Ok(artifact)
}

fn package_artifact_json(artifact: &PackageArtifact) -> serde_json::Value {
    serde_json::json!({
        "name": "agentic-harness",
        "version": artifact.version,
        "artifact": artifact.artifact_name,
        "directory": artifact.dir.display().to_string(),
        "binary": artifact.binary_name,
        "archive": artifact.archive_name,
        "binaryPath": artifact.binary.display().to_string(),
        "archivePath": artifact.archive.display().to_string(),
        "manifestPath": artifact.manifest.display().to_string(),
        "checksumsPath": artifact.checksums.display().to_string(),
        "platform": {
            "os": artifact.os,
            "arch": artifact.arch,
        },
        "sizeBytes": artifact.size_bytes,
        "sha256": artifact.sha256,
        "archiveSizeBytes": artifact.archive_size_bytes,
        "archiveSha256": artifact.archive_sha256,
    })
}

fn package_archive_manifest_json(
    version: &str,
    os: &str,
    arch: &str,
    artifact_name: &str,
    binary_name: &str,
    size_bytes: u64,
    sha256: &str,
) -> serde_json::Value {
    serde_json::json!({
        "name": "agentic-harness",
        "version": version,
        "artifact": artifact_name,
        "binary": binary_name,
        "platform": {
            "os": os,
            "arch": arch,
        },
        "sizeBytes": size_bytes,
        "sha256": sha256,
    })
}

struct TarEntry {
    name: String,
    mode: u32,
    bytes: Vec<u8>,
}

fn write_tar_archive(path: &Path, entries: &[TarEntry]) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = fs::File::create(path)?;
    for entry in entries {
        write_tar_file(&mut file, &entry.name, entry.mode, &entry.bytes)?;
    }
    file.write_all(&[0u8; 1024])?;
    Ok(())
}

fn write_tar_file(
    file: &mut fs::File,
    name: &str,
    mode: u32,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    if name.len() > 100 {
        return Err(format!("tar entry name is too long: {name}").into());
    }
    let mut header = [0u8; 512];
    write_tar_str(&mut header[0..100], name);
    write_tar_octal(&mut header[100..108], mode as u64);
    write_tar_octal(&mut header[108..116], 0);
    write_tar_octal(&mut header[116..124], 0);
    write_tar_octal(&mut header[124..136], bytes.len() as u64);
    write_tar_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = b'0';
    write_tar_str(&mut header[257..263], "ustar");
    write_tar_str(&mut header[263..265], "00");
    let checksum = header.iter().map(|byte| *byte as u32).sum::<u32>();
    let checksum = format!("{checksum:06o}\0 ");
    header[148..156].copy_from_slice(checksum.as_bytes());

    file.write_all(&header)?;
    file.write_all(bytes)?;
    let padding = (512 - (bytes.len() % 512)) % 512;
    if padding > 0 {
        file.write_all(&vec![0u8; padding])?;
    }
    Ok(())
}

fn write_tar_str(field: &mut [u8], value: &str) {
    let bytes = value.as_bytes();
    let len = bytes.len().min(field.len());
    field[..len].copy_from_slice(&bytes[..len]);
}

fn write_tar_octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let value = format!("{value:0width$o}");
    field[..width].copy_from_slice(value.as_bytes());
    field[width] = 0;
}

fn make_executable(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

fn sha256_hex(input: &[u8]) -> String {
    sha256(input)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()
}

fn sha256(input: &[u8]) -> [u8; 32] {
    const H0: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];

    let bit_len = (input.len() as u64) * 8;
    let mut message = input.to_vec();
    message.push(0x80);
    while (message.len() % 64) != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    let mut hash = H0;
    for chunk in message.chunks(64) {
        let mut words = [0u32; 64];
        for (index, bytes) in chunk.chunks(4).take(16).enumerate() {
            words[index] = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        }
        for index in 16..64 {
            let s0 = words[index - 15].rotate_right(7)
                ^ words[index - 15].rotate_right(18)
                ^ (words[index - 15] >> 3);
            let s1 = words[index - 2].rotate_right(17)
                ^ words[index - 2].rotate_right(19)
                ^ (words[index - 2] >> 10);
            words[index] = words[index - 16]
                .wrapping_add(s0)
                .wrapping_add(words[index - 7])
                .wrapping_add(s1);
        }

        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = hash;
        for index in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[index])
                .wrapping_add(words[index]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        for (slot, value) in hash.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }

    let mut output = [0u8; 32];
    for (chunk, word) in output.chunks_mut(4).zip(hash) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(unix)]
fn file_is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn file_is_executable(path: &Path) -> bool {
    path.is_file()
}

fn guide_steps(
    workspace: &Path,
    environment: LlmAuthoringEnvironment,
    template: &str,
    prompt: &str,
) -> Vec<GuideStep> {
    let workspace_arg = quote_cli_arg(&workspace.display().to_string());
    let template_arg = quote_cli_arg(template);
    let report = doctor_report(workspace);
    let has_brief = workspace
        .join(".agentic-harness/template-briefs")
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    vec![
        GuideStep {
            id: "install-cli",
            title: "Install CLI",
            detail: "make the local binary available before starting a project".to_string(),
            command: "./scripts/install.sh".to_string(),
            status: "manual",
        },
        GuideStep {
            id: "setup-llm",
            title: "Set up LLM authoring",
            detail: format!(
                "install template-authoring context for {}",
                environment.label()
            ),
            command: format!(
                "agentic-harness setup llm --workspace {workspace_arg} --env {}",
                environment.slug()
            ),
            status: if llm_authoring_installed(workspace) {
                "done"
            } else {
                "next"
            },
        },
        GuideStep {
            id: "author-template",
            title: "Create reusable template",
            detail: "ask the LLM to create a valid template pack instead of hand-writing SDK glue"
                .to_string(),
            command: format!(
                "agentic-harness template author {template_arg} --workspace {workspace_arg} --env {} --prompt {} --open",
                environment.slug(),
                quote_cli_value(prompt)
            ),
            status: if has_brief { "done" } else { "next" },
        },
        GuideStep {
            id: "start-coding",
            title: "Start coding",
            detail: "run the coding-agent loop against the workspace".to_string(),
            command: format!(
                "agentic-harness code --workspace {workspace_arg} --prompt {} --llm {}",
                quote_cli_value("Describe the software change"),
                environment.slug()
            ),
            status: if report.required_ready() {
                "ready"
            } else {
                "needs-project"
            },
        },
        GuideStep {
            id: "inspect-result",
            title: "Inspect result",
            detail: "read the latest Markdown run summary saved by the coding-agent loop"
                .to_string(),
            command: format!("agentic-harness inspect --workspace {workspace_arg}"),
            status: "after-run",
        },
    ]
}

fn format_guide_report(
    workspace: &Path,
    environment: LlmAuthoringEnvironment,
    template: &str,
    steps: &[GuideStep],
) -> String {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut out = String::new();
    out.push_str("Agentic Harness Start Guide\n");
    out.push_str(&format!("workspace: {}\n", workspace_display.display()));
    out.push_str(&format!("environment: {}\n", environment.label()));
    out.push_str(&format!("template: {template}\n\n"));
    for (index, step) in steps.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} [{}]\n   {}\n   {}\n",
            index + 1,
            step.title,
            step.status,
            step.command,
            step.detail
        ));
    }
    out.push_str("\nNext: run the first command whose status is not done.\n");
    out
}

fn format_guide_json(
    workspace: &Path,
    environment: LlmAuthoringEnvironment,
    template: &str,
    steps: &[GuideStep],
) -> Result<String, Box<dyn std::error::Error>> {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let value = serde_json::json!({
        "workspace": workspace_display.display().to_string(),
        "environment": environment.slug(),
        "template": template,
        "steps": steps.iter().map(|step| {
            serde_json::json!({
                "id": step.id,
                "title": step.title,
                "status": step.status,
                "command": step.command,
                "detail": step.detail,
            })
        }).collect::<Vec<_>>(),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn quote_cli_arg(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | '+' | '='))
    {
        value.to_string()
    } else {
        quote_cli_value(value)
    }
}

fn quote_cli_value(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn doctor_report(workspace: &Path) -> DoctorReport {
    let workspace_display = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let workspace_ok = workspace.is_dir();
    let cargo_toml = workspace.join("Cargo.toml");
    let main_rs = workspace.join("src/main.rs");
    let agents_md = workspace.join("AGENTS.md");
    let roles_dir = first_existing_dir(workspace, &[".agentic-harness/roles", "roles"]);
    let skills_dir = first_existing_dir(workspace, &[".agents/skills", "skills"]);
    let sandbox = sandbox_doctor_check(workspace);
    let llm_authoring = llm_authoring_doctor_check(workspace);
    let llm_tools = llm_tools_doctor_check();

    let mut checks = vec![
            DoctorCheck {
                label: "workspace",
                ok: workspace_ok,
                required: true,
                detail: if workspace_ok {
                    "directory found".to_string()
                } else {
                    format!("directory not found at {}", workspace.display())
                },
                fix: "Create a project with: agentic-harness new ./my-agent --name my-agent --template hello",
            },
            DoctorCheck {
                label: "Cargo.toml",
                ok: cargo_toml.exists(),
                required: true,
                detail: if cargo_toml.exists() {
                    "Rust package manifest found".to_string()
                } else {
                    format!("No Cargo.toml found at {}", cargo_toml.display())
                },
                fix: "Create a project with: agentic-harness new ./my-agent --name my-agent --template hello",
            },
            DoctorCheck {
                label: "src/main.rs",
                ok: main_rs.exists(),
                required: true,
                detail: if main_rs.exists() {
                    "native agent entrypoint found".to_string()
                } else {
                    format!("No src/main.rs found at {}", main_rs.display())
                },
                fix: "Add a Rust entrypoint or scaffold one with: agentic-harness new ./my-agent --name my-agent --template hello",
            },
            DoctorCheck {
                label: "AGENTS.md",
                ok: agents_md.exists(),
                required: false,
                detail: if agents_md.exists() {
                    "workspace instructions found".to_string()
                } else {
                    "workspace instructions are missing".to_string()
                },
                fix: "Add AGENTS.md with the behavior and repo rules your agent should follow.",
            },
            DoctorCheck {
                label: "roles",
                ok: roles_dir.is_some(),
                required: false,
                detail: roles_dir
                    .as_ref()
                    .map(|path| format!("{} found", path.display()))
                    .unwrap_or_else(|| {
                        "no .agentic-harness/roles or roles directory found".to_string()
                    }),
                fix: "Add role markdown files under .agentic-harness/roles or roles.",
            },
            DoctorCheck {
                label: "skills",
                ok: skills_dir.is_some(),
                required: false,
                detail: skills_dir
                    .as_ref()
                    .map(|path| format!("{} found", path.display()))
                    .unwrap_or_else(|| "no .agents/skills or skills directory found".to_string()),
                fix: "Add skill folders under .agents/skills or skills when your agent needs reusable procedures.",
            },
        ];
    checks.push(sandbox);
    checks.push(llm_authoring);
    checks.push(llm_tools);

    DoctorReport {
        workspace: workspace_display,
        checks,
    }
}

fn first_existing_dir(workspace: &Path, candidates: &[&str]) -> Option<PathBuf> {
    candidates
        .iter()
        .map(|candidate| workspace.join(candidate))
        .find(|path| path.is_dir())
}

fn llm_authoring_installed(workspace: &Path) -> bool {
    workspace
        .join(".agents/skills/agentic-harness-template/SKILL.md")
        .exists()
        && workspace
            .join(".agentic-harness/llm-authoring.toml")
            .exists()
}

fn llm_authoring_doctor_check(workspace: &Path) -> DoctorCheck {
    DoctorCheck {
        label: "llm authoring",
        ok: llm_authoring_installed(workspace),
        required: false,
        detail: if llm_authoring_installed(workspace) {
            "template authoring context installed".to_string()
        } else {
            "template authoring context is not installed".to_string()
        },
        fix: "Install it with: agentic-harness setup llm --workspace .",
    }
}

fn llm_tools_doctor_check() -> DoctorCheck {
    let tools = llm_tool_availability();
    let ready = tools.iter().filter(|tool| tool.ready).count();
    DoctorCheck {
        label: "llm tools",
        ok: ready > 0,
        required: false,
        detail: llm_tool_status_from(&tools),
        fix: "Install Claude Code, Codex, Cursor, or Wind Server before using `agentic-harness code --llm auto`.",
    }
}

fn sandbox_doctor_check(workspace: &Path) -> DoctorCheck {
    let config = workspace.join(".agentic-harness/sandbox.toml");
    let target = fs::read_to_string(&config)
        .ok()
        .and_then(|content| parse_manifest_string(&content, "target").ok())
        .unwrap_or_else(|| "local".to_string());
    if target == "local" {
        let smoke = run_local_sandbox_smoke(workspace);
        DoctorCheck {
            label: "sandbox",
            ok: smoke.ok,
            required: false,
            detail: if config.exists() {
                format!("local checkout configured; {}", smoke.detail)
            } else {
                format!("local checkout default; {}", smoke.detail)
            },
            fix: "Configure it with: agentic-harness setup sandbox --target local",
        }
    } else if config.exists() {
        let sandbox_config = load_sandbox_config(workspace);
        let smoke = run_sandbox_smoke(workspace, &sandbox_config);
        DoctorCheck {
            label: "sandbox",
            ok: smoke.ok,
            required: false,
            detail: format!("remote sandbox target {target:?}; {}", smoke.detail),
            fix: "Configure it with: agentic-harness setup sandbox --target local or print remote instructions with --print",
        }
    } else {
        DoctorCheck {
            label: "sandbox",
            ok: false,
            required: false,
            detail: format!("remote sandbox target {target:?} configured"),
            fix: "Configure it with: agentic-harness setup sandbox --target local or print remote instructions with --print",
        }
    }
}

fn format_doctor_report(report: &DoctorReport, color: bool) -> String {
    let mut out = String::new();
    if color {
        out.push_str(&format!(
            "\n{PURPLE}◇{RESET}  {BOLD}{WHITE}Agentic Harness Doctor{RESET}\n{MUTED}│{RESET}  workspace: {WHITE}{}{RESET}\n{MUTED}│{RESET}\n",
            report.workspace.display()
        ));
    } else {
        out.push_str("Agentic Harness Doctor\n");
        out.push_str(&format!("workspace: {}\n\n", report.workspace.display()));
    }

    for check in &report.checks {
        if color {
            let marker = if check.ok {
                format!("{GREEN}✓{RESET} ok")
            } else if check.required {
                format!("{ORANGE}!{RESET} missing")
            } else {
                format!("{ORANGE}!{RESET} optional")
            };
            out.push_str(&format!(
                "{MUTED}│{RESET}  {WHITE}{:<11}{RESET} {marker}  {MUTED}{}{RESET}\n",
                check.label, check.detail
            ));
            if !check.ok {
                out.push_str(&format!(
                    "{MUTED}│{RESET}  {MUTED}fix:{RESET} {}\n",
                    check.fix
                ));
            }
        } else {
            let status = if check.ok { "ok" } else { "missing" };
            out.push_str(&format!("{}: {} - {}\n", check.label, status, check.detail));
            if !check.ok {
                out.push_str(&format!("fix: {}\n", check.fix));
            }
        }
    }

    if color {
        if report.required_ready() {
            out.push_str(&format!(
                "{MUTED}│{RESET}\n{MUTED}└{RESET}  {GREEN}ready{RESET}  {MUTED}workspace can run with agentic-harness{RESET}\n"
            ));
            if report.has_optional_warnings() {
                out.push_str(&format!(
                    "{MUTED}   {RESET}  {ORANGE}!{RESET} optional setup files are missing\n"
                ));
            }
        } else {
            out.push_str(&format!(
                "{MUTED}│{RESET}\n{MUTED}└{RESET}  {ORANGE}not ready{RESET}  {MUTED}missing required files{RESET}\n"
            ));
        }
    } else if report.required_ready() {
        out.push_str("\nready: workspace can run with agentic-harness\n");
        if report.has_optional_warnings() {
            out.push_str("warnings: optional setup files are missing\n");
        }
    } else {
        out.push_str("\nnot ready: missing required files\n");
        out.push_str("next: agentic-harness new ./my-agent --name my-agent --template hello\n");
    }

    out
}

fn format_doctor_json(report: &DoctorReport) -> Result<String, Box<dyn std::error::Error>> {
    let value = serde_json::json!({
        "workspace": report.workspace.display().to_string(),
        "requiredReady": report.required_ready(),
        "optionalWarnings": report.has_optional_warnings(),
        "nextCommand": if report.required_ready() {
            format!("agentic-harness code --workspace {}", report.workspace.display())
        } else {
            "agentic-harness new ./my-agent --name my-agent --template hello".to_string()
        },
        "checks": report.checks.iter().map(|check| {
            serde_json::json!({
                "label": check.label,
                "ok": check.ok,
                "required": check.required,
                "detail": check.detail,
                "fix": check.fix,
            })
        }).collect::<Vec<_>>(),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn format_dashboard_report(
    workspace: &Path,
    report: &DoctorReport,
    color: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut out = String::new();
    if color {
        out.push_str(&format!(
            "\n{PURPLE}◇{RESET}  {BOLD}{WHITE}Agentic Harness Dashboard{RESET}\n{MUTED}│{RESET}  workspace: {WHITE}{}{RESET}\n",
            report.workspace.display()
        ));
    } else {
        out.push_str("Agentic Harness Dashboard\n");
        out.push_str(&format!("Workspace: {}\n", report.workspace.display()));
    }

    push_dashboard_line(
        &mut out,
        color,
        "Required checks",
        if report.required_ready() {
            "ready"
        } else {
            "blocked"
        },
    );
    push_dashboard_line(
        &mut out,
        color,
        "Optional warnings",
        if report.has_optional_warnings() {
            "present"
        } else {
            "clear"
        },
    );

    out.push('\n');
    out.push_str("Workspace checks\n");
    for check in &report.checks {
        out.push_str(&format!(
            "  - {}: {} - {}\n",
            check.label,
            if check.ok { "ok" } else { "needs attention" },
            check.detail
        ));
    }

    out.push('\n');
    out.push_str("Templates\n");
    for entry in dashboard_template_entries(workspace)? {
        out.push_str(&format!("  - {entry}\n"));
    }

    out.push('\n');
    out.push_str("Template briefs\n");
    for entry in dashboard_template_brief_entries(workspace)? {
        out.push_str(&format!("  - {entry}\n"));
    }

    out.push('\n');
    out.push_str("Latest coding run\n");
    if let Some(run) = latest_coding_run_json(workspace) {
        let loop_lines = latest_coding_loop_lines(&run);
        if loop_lines.is_empty() {
            out.push_str("  - loop: unavailable\n");
        } else {
            for line in loop_lines {
                out.push_str(&format!("  - {line}\n"));
            }
        }
        let changed_files = latest_coding_changed_files(&run);
        if changed_files.is_empty() {
            out.push_str("  - changed files: none\n");
        } else {
            out.push_str(&format!(
                "  - changed files: {}\n",
                changed_files.join(", ")
            ));
        }
    } else {
        out.push_str("  - none\n");
    }

    let hosting = hosting_status(workspace);
    out.push('\n');
    out.push_str("Local hosting\n");
    out.push_str(&format!("  - addr: {}\n", hosting.addr));
    if let Some(base_url) = &hosting.base_url {
        out.push_str(&format!("  - base URL: {base_url}\n"));
    }
    if let Some(health_url) = &hosting.health_url {
        out.push_str(&format!("  - health: {health_url}\n"));
    }
    out.push_str(&format!(
        "  - status: {} - {}\n",
        if hosting.servable { "ready" } else { "blocked" },
        hosting.detail
    ));
    out.push_str(&format!("  - start: {}\n", hosting.start_command));
    out.push_str(&format!("  - status command: {}\n", hosting.status_command));

    let config = load_sandbox_config(workspace);
    let smoke = run_sandbox_smoke(workspace, &config);
    out.push('\n');
    out.push_str("Sandbox\n");
    out.push_str(&format!("  - target: {}\n", config.target));
    out.push_str(&format!("  - cwd: {}\n", config.cwd));
    if config.target == "local" || config.endpoint.is_some() {
        out.push_str(&format!(
            "  - smoke: {} - {}\n",
            if smoke.ok { "ok" } else { "failed" },
            smoke.detail
        ));
    } else {
        out.push_str("  - operations: connector required for remote target\n");
    }

    out.push('\n');
    out.push_str("Recent sandbox logs\n");
    let logs = recent_sandbox_logs(workspace, 5);
    if logs.is_empty() {
        out.push_str("  - none\n");
    } else {
        for line in logs {
            out.push_str(&format!("  - {line}\n"));
        }
    }

    out.push('\n');
    out.push_str("Next commands\n");
    for command in dashboard_next_commands(workspace) {
        out.push_str(&format!("  - {command}\n"));
    }
    Ok(out)
}

fn format_dashboard_json(
    workspace: &Path,
    report: &DoctorReport,
) -> Result<String, Box<dyn std::error::Error>> {
    let value = serde_json::json!({
        "workspace": report.workspace.display().to_string(),
        "requiredReady": report.required_ready(),
        "optionalWarnings": report.has_optional_warnings(),
        "checks": report.checks.iter().map(|check| {
            serde_json::json!({
                "label": check.label,
                "ok": check.ok,
                "required": check.required,
                "detail": check.detail,
                "fix": check.fix,
            })
        }).collect::<Vec<_>>(),
        "templates": dashboard_template_json_entries(workspace)?,
        "templateBriefs": dashboard_template_brief_json_entries(workspace)?,
        "latestCodingRun": latest_coding_run_json(workspace),
        "localHosting": dashboard_hosting_json(workspace),
        "sandbox": dashboard_sandbox_json(workspace),
        "recentSandboxLogs": recent_sandbox_logs(workspace, 5),
        "nextCommands": dashboard_next_commands(workspace),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn push_dashboard_line(out: &mut String, color: bool, label: &str, value: &str) {
    if color {
        out.push_str(&format!(
            "{MUTED}│{RESET}  {WHITE}{label}{RESET}: {value}\n"
        ));
    } else {
        out.push_str(&format!("{label}: {value}\n"));
    }
}

fn dashboard_template_entries(workspace: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut entries = Vec::new();
    entries.push(format!(
        "built-ins: {}",
        BUILT_IN_TEMPLATES
            .iter()
            .map(|template| template.name())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let installed = workspace.join(".agentic-harness/templates");
    if installed.is_dir() {
        for scope in [
            TemplateScope::Workspace,
            TemplateScope::User,
            TemplateScope::Team,
        ] {
            for (name, path) in template_scope_entries(workspace, scope)? {
                match load_template_pack(&path).and_then(|pack| {
                    validate_template_pack(&pack)?;
                    Ok(pack)
                }) {
                    Ok(pack) => entries.push(format!(
                        "{}:{} (agent {}, version {}, {})",
                        scope.source(),
                        name,
                        pack.agent_name,
                        pack.version,
                        pack.description
                    )),
                    Err(err) => entries.push(format!("{} (invalid: {err})", path.display())),
                }
            }
        }
    } else {
        entries.push("installed: none".to_string());
    }
    Ok(entries)
}

fn dashboard_template_json_entries(
    workspace: &Path,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let mut entries = BUILT_IN_TEMPLATES
        .iter()
        .copied()
        .map(|template| {
            serde_json::json!({
                "name": template.name(),
                "agent": template.agent_name(),
                "version": env!("CARGO_PKG_VERSION"),
                "description": template.description(),
                "source": "built-in",
                "valid": true,
            })
        })
        .collect::<Vec<_>>();

    for scope in [
        TemplateScope::Workspace,
        TemplateScope::User,
        TemplateScope::Team,
    ] {
        for (name, path) in template_scope_entries(workspace, scope)? {
            let entry = match load_template_pack(&path).and_then(|pack| {
                validate_template_pack(&pack)?;
                Ok(pack)
            }) {
                Ok(pack) => serde_json::json!({
                    "name": pack.name,
                    "directory": name,
                    "agent": pack.agent_name,
                    "version": pack.version,
                    "description": pack.description,
                    "source": scope.source(),
                    "path": path.display().to_string(),
                    "valid": true,
                }),
                Err(err) => serde_json::json!({
                    "name": name,
                    "source": scope.source(),
                    "path": path.display().to_string(),
                    "valid": false,
                    "error": err.to_string(),
                }),
            };
            entries.push(entry);
        }
    }

    Ok(entries)
}

fn dashboard_template_brief_entries(
    workspace: &Path,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let briefs = workspace.join(".agentic-harness/template-briefs");
    if !briefs.is_dir() {
        return Ok(vec!["none".to_string()]);
    }
    let mut paths = fs::read_dir(briefs)?
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Ok(vec!["none".to_string()]);
    }
    Ok(paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("brief.md");
            let content = fs::read_to_string(&path).unwrap_or_default();
            format!("{name} - {}", brief_request_summary(&content))
        })
        .collect())
}

fn dashboard_template_brief_json_entries(
    workspace: &Path,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let briefs = workspace.join(".agentic-harness/template-briefs");
    if !briefs.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = fs::read_dir(briefs)?
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    paths.sort();
    Ok(paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("brief.md")
                .to_string();
            let content = fs::read_to_string(&path).unwrap_or_default();
            serde_json::json!({
                "name": name,
                "path": path.display().to_string(),
                "request": brief_request_summary(&content),
            })
        })
        .collect())
}

fn dashboard_sandbox_json(workspace: &Path) -> serde_json::Value {
    let config = load_sandbox_config(workspace);
    let smoke = run_sandbox_smoke(workspace, &config);
    let operations = if config.target == "local" {
        "local checkout"
    } else if config.endpoint.is_some() {
        "http endpoint configured"
    } else {
        "connector required for remote target"
    };
    serde_json::json!({
        "target": config.target,
        "cwd": config.cwd,
        "endpoint": config.endpoint,
        "operations": operations,
        "smoke": {
            "ok": smoke.ok,
            "detail": smoke.detail,
        },
    })
}

fn latest_coding_run_json(workspace: &Path) -> Option<serde_json::Value> {
    fs::read_to_string(workspace.join(DEFAULT_CODING_SUMMARY_JSON_PATH))
        .ok()
        .and_then(|content| serde_json::from_str(&content).ok())
}

fn latest_coding_loop_lines(run: &serde_json::Value) -> Vec<String> {
    run.get("loop")
        .and_then(|loop_entries| loop_entries.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let phase = entry.get("phase")?.as_str()?;
            let status = entry.get("status")?.as_str()?;
            Some(format!("{phase}: {status}"))
        })
        .collect()
}

fn latest_coding_changed_files(run: &serde_json::Value) -> Vec<String> {
    run.get("changedFiles")
        .and_then(|files| files.as_array())
        .into_iter()
        .flatten()
        .filter_map(|file| file.as_str().map(ToOwned::to_owned))
        .collect()
}

fn dashboard_next_commands(workspace: &Path) -> Vec<String> {
    vec![
        format!("agentic-harness start --workspace {}", workspace.display()),
        format!(
            "agentic-harness template list --workspace {} --verbose",
            workspace.display()
        ),
        format!(
            "agentic-harness sandbox status --workspace {}",
            workspace.display()
        ),
        format!(
            "agentic-harness hosting status --workspace {}",
            workspace.display()
        ),
        format!(
            "agentic-harness doctor --workspace {} --plain",
            workspace.display()
        ),
    ]
}

fn brief_request_summary(content: &str) -> String {
    let mut in_request = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "## User Request" {
            in_request = true;
            continue;
        }
        if in_request {
            if line.starts_with("## ") {
                break;
            }
            if !line.is_empty() {
                return trim_inline(line, 120);
            }
        }
    }
    "no request summary".to_string()
}

fn trim_inline(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut trimmed = value
        .chars()
        .take(limit.saturating_sub(3))
        .collect::<String>();
    trimmed.push_str("...");
    trimmed
}

fn recent_sandbox_logs(workspace: &Path, limit: usize) -> Vec<String> {
    fs::read_to_string(sandbox_log_path(workspace))
        .map(|content| {
            let mut lines = content
                .lines()
                .rev()
                .take(limit)
                .map(str::to_string)
                .collect::<Vec<_>>();
            lines.reverse();
            lines
        })
        .unwrap_or_default()
}

fn wizard_command(workspace: &Path, plain: bool) -> Result<u8, Box<dyn std::error::Error>> {
    if plain {
        print!("{}", wizard_dashboard(workspace));
        return Ok(0);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(
            "agentic-harness wizard is an interactive wizard. Use `agentic-harness wizard --plain` in non-TTY shells."
                .into(),
        );
    }
    run_interactive_wizard(workspace)
}

fn run_interactive_wizard(workspace: &Path) -> Result<u8, Box<dyn std::error::Error>> {
    loop {
        println!("{}", styled_wizard_dashboard(workspace));
        let input = prompt_wizard("Choose workflow")?;
        match input.trim() {
            "q" | "Q" => {
                println!("{MUTED}bye.{RESET}");
                return Ok(0);
            }
            value => {
                let Some(step) = find_wizard_step(value) else {
                    eprintln!(
                        "Unknown workflow. Enter {}, or q to quit.",
                        wizard_key_hint()
                    );
                    continue;
                };
                if run_wizard_step(step, workspace)? == WizardNav::Quit {
                    return Ok(0);
                }
            }
        }
    }
}

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const PURPLE: &str = "\x1b[38;2;191;90;242m";
const BLUE: &str = "\x1b[38;2;10;132;255m";
const GREEN: &str = "\x1b[38;2;48;209;88m";
const ORANGE: &str = "\x1b[38;2;255;159;10m";
const MUTED: &str = "\x1b[38;2;99;99;102m";
const WHITE: &str = "\x1b[38;2;245;245;247m";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardNav {
    Back,
    Quit,
}

#[derive(Debug)]
struct WizardStep {
    key: &'static str,
    title: &'static str,
    detail: &'static str,
    choices: &'static [WizardChoice],
}

#[derive(Debug)]
struct WizardChoice {
    key: &'static str,
    title: &'static str,
    detail: &'static str,
    command: &'static str,
    action: Option<WizardAction>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WizardAction {
    CodeCurrent,
    CodeCurrentWithAutoLlm,
    CodeWorkspace,
    ScaffoldCodingProject,
    TemplateInit,
    TemplateAuthor,
    TemplateList,
    TemplateSearch,
    TemplateShow,
    TemplateValidate,
    TemplateExport,
    TemplateInstall,
    TemplateImport,
    SetupLlm(LlmAuthoringEnvironment),
    SetupLlmAuto,
    SetupSandbox(SandboxTarget),
    SetupSandboxRemote(SandboxTarget),
    SandboxStatus,
    SandboxExec,
    SandboxSync,
    SandboxLogs,
    SandboxCleanup,
    RunHello,
    RunCustom,
    RunWithEnv,
    SetupHosting,
    HostingStatus,
    HostCurrent,
    HostCurrentDev,
    ResultCurrent,
    ResultCustom,
    DashboardCurrent,
    DashboardCustom,
    DoctorCurrent,
    DoctorExample,
    DoctorCustom,
}

static CODE_CHOICES: [WizardChoice; 4] = [
    WizardChoice {
        key: "1",
        title: "Start coding here",
        detail: "run the coding agent from the current workspace",
        command: "agentic-harness code",
        action: Some(WizardAction::CodeCurrent),
    },
    WizardChoice {
        key: "2",
        title: "Start coding with detected LLM",
        detail: "open a generated coding brief in the first available coding CLI",
        command: "agentic-harness code --llm auto",
        action: Some(WizardAction::CodeCurrentWithAutoLlm),
    },
    WizardChoice {
        key: "3",
        title: "Start coding in another workspace",
        detail: "pick a workspace and enter a prompt",
        command: "prompts for workspace and prompt",
        action: Some(WizardAction::CodeWorkspace),
    },
    WizardChoice {
        key: "4",
        title: "Create coding agent",
        detail: "scaffold the default software-agent template",
        command: "agentic-harness new ./my-agent --template coding",
        action: Some(WizardAction::ScaffoldCodingProject),
    },
];

static TEMPLATE_CHOICES: [WizardChoice; 9] = [
    WizardChoice {
        key: "1",
        title: "Create template pack",
        detail: "generate a reusable starter pack for an LLM to customize",
        command: "agentic-harness tpl create ./my-template --name my-template",
        action: Some(WizardAction::TemplateInit),
    },
    WizardChoice {
        key: "2",
        title: "Create with LLM",
        detail: "write a handoff brief for Claude Code, Codex, Cursor, or Wind Server",
        command: "agentic-harness template author my-template --env codex --prompt ... --open",
        action: Some(WizardAction::TemplateAuthor),
    },
    WizardChoice {
        key: "3",
        title: "List templates",
        detail: "show built-in and installed template metadata",
        command: "agentic-harness templates ls --verbose",
        action: Some(WizardAction::TemplateList),
    },
    WizardChoice {
        key: "4",
        title: "Search templates",
        detail: "find built-in, workspace, user, or team packs by text",
        command: "agentic-harness templates search review",
        action: Some(WizardAction::TemplateSearch),
    },
    WizardChoice {
        key: "5",
        title: "Preview template",
        detail: "inspect template purpose, agent, files, and sample payload",
        command: "agentic-harness tpl preview coding",
        action: Some(WizardAction::TemplateShow),
    },
    WizardChoice {
        key: "6",
        title: "Validate template pack",
        detail: "check manifest, files, and sample payload",
        command: "agentic-harness templates check path",
        action: Some(WizardAction::TemplateValidate),
    },
    WizardChoice {
        key: "7",
        title: "Export template pack",
        detail: "copy a reusable pack to a shareable folder",
        command: "agentic-harness template export coding --output ./coding-template",
        action: Some(WizardAction::TemplateExport),
    },
    WizardChoice {
        key: "8",
        title: "Install template pack",
        detail: "copy a local pack into workspace, user, or team registry",
        command: "agentic-harness tpl add path --scope workspace",
        action: Some(WizardAction::TemplateInstall),
    },
    WizardChoice {
        key: "9",
        title: "Import template pack",
        detail: "add an exported pack to workspace, user, or team registry",
        command: "agentic-harness tpl use path --scope workspace",
        action: Some(WizardAction::TemplateImport),
    },
];

static LLM_CHOICES: [WizardChoice; 5] = [
    WizardChoice {
        key: "1",
        title: "Claude Code",
        detail: "install template-authoring context for Claude Code",
        command: "agentic-harness setup llm --env claude-code",
        action: Some(WizardAction::SetupLlm(LlmAuthoringEnvironment::ClaudeCode)),
    },
    WizardChoice {
        key: "2",
        title: "Codex",
        detail: "install template-authoring context for Codex",
        command: "agentic-harness setup llm --env codex",
        action: Some(WizardAction::SetupLlm(LlmAuthoringEnvironment::Codex)),
    },
    WizardChoice {
        key: "3",
        title: "Cursor",
        detail: "install template-authoring context for Cursor",
        command: "agentic-harness setup llm --env cursor",
        action: Some(WizardAction::SetupLlm(LlmAuthoringEnvironment::Cursor)),
    },
    WizardChoice {
        key: "4",
        title: "Wind Server",
        detail: "install template-authoring context for Wind Server",
        command: "agentic-harness setup llm --env wind-server",
        action: Some(WizardAction::SetupLlm(LlmAuthoringEnvironment::WindServer)),
    },
    WizardChoice {
        key: "5",
        title: "Detect automatically",
        detail: "use the default environment detection",
        command: "agentic-harness setup llm --env auto",
        action: Some(WizardAction::SetupLlmAuto),
    },
];

static SANDBOX_CHOICES: [WizardChoice; 10] = [
    WizardChoice {
        key: "1",
        title: "Local checkout",
        detail: "run shell and file operations in the current project",
        command: "agentic-harness setup sandbox --target local",
        action: Some(WizardAction::SetupSandbox(SandboxTarget::Local)),
    },
    WizardChoice {
        key: "2",
        title: "Sandbox status",
        detail: "show target, cwd, and smoke-check status",
        command: "agentic-harness sandbox status",
        action: Some(WizardAction::SandboxStatus),
    },
    WizardChoice {
        key: "3",
        title: "Run sandbox command",
        detail: "execute a shell command in the local sandbox",
        command: "agentic-harness sandbox exec command",
        action: Some(WizardAction::SandboxExec),
    },
    WizardChoice {
        key: "4",
        title: "Sync into sandbox",
        detail: "copy a host file or folder into the local sandbox",
        command: "agentic-harness sandbox sync source destination",
        action: Some(WizardAction::SandboxSync),
    },
    WizardChoice {
        key: "5",
        title: "Sandbox logs",
        detail: "show local sandbox operation history",
        command: "agentic-harness sandbox logs",
        action: Some(WizardAction::SandboxLogs),
    },
    WizardChoice {
        key: "6",
        title: "Clean sandbox path",
        detail: "remove a file or folder from the local sandbox",
        command: "agentic-harness sandbox rm path --recursive",
        action: Some(WizardAction::SandboxCleanup),
    },
    WizardChoice {
        key: "7",
        title: "Vercel Sandbox",
        detail: "print hosted sandbox connector instructions",
        command: "agentic-harness setup sandbox --target vercel --print",
        action: Some(WizardAction::SetupSandboxRemote(SandboxTarget::Vercel)),
    },
    WizardChoice {
        key: "8",
        title: "Daytona",
        detail: "print remote sandbox connector instructions",
        command: "agentic-harness setup sandbox --target daytona --print",
        action: Some(WizardAction::SetupSandboxRemote(SandboxTarget::Daytona)),
    },
    WizardChoice {
        key: "9",
        title: "E2B",
        detail: "print cloud sandbox connector instructions",
        command: "agentic-harness setup sandbox --target e2b --print",
        action: Some(WizardAction::SetupSandboxRemote(SandboxTarget::E2b)),
    },
    WizardChoice {
        key: "10",
        title: "Custom remote sandbox",
        detail: "print the generic SessionEnv connector contract",
        command: "agentic-harness setup sandbox --target custom --print",
        action: Some(WizardAction::SetupSandboxRemote(SandboxTarget::Custom)),
    },
];

static RUN_CHOICES: [WizardChoice; 7] = [
    WizardChoice {
        key: "1",
        title: "Run hello locally",
        detail: "invoke one native agent with a JSON payload",
        command: "agentic-harness run hello --workspace examples/hello-world --id demo --payload '{\"name\":\"Ada\"}'",
        action: Some(WizardAction::RunHello),
    },
    WizardChoice {
        key: "2",
        title: "Run from your checkout",
        detail: "point the runtime at the current project",
        command: "prompts for agent, workspace, id, and JSON payload",
        action: Some(WizardAction::RunCustom),
    },
    WizardChoice {
        key: "3",
        title: "Run with environment files",
        detail: "load model keys or sandbox credentials from .env",
        command: "prompts for agent, workspace, id, payload, and env files",
        action: Some(WizardAction::RunWithEnv),
    },
    WizardChoice {
        key: "4",
        title: "Configure local hosting",
        detail: "save the loopback address for the native HTTP server",
        command: "agentic-harness setup hosting --workspace . --addr 127.0.0.1:3583",
        action: Some(WizardAction::SetupHosting),
    },
    WizardChoice {
        key: "5",
        title: "Hosting status",
        detail: "show URLs, endpoints, and local server capabilities",
        command: "agentic-harness hosting status --workspace .",
        action: Some(WizardAction::HostingStatus),
    },
    WizardChoice {
        key: "6",
        title: "Start local host",
        detail: "serve the native app over local HTTP",
        command: "agentic-harness host --workspace .",
        action: Some(WizardAction::HostCurrent),
    },
    WizardChoice {
        key: "7",
        title: "Start dev host",
        detail: "serve locally with watch/reload",
        command: "agentic-harness host --workspace . --dev",
        action: Some(WizardAction::HostCurrentDev),
    },
];

static CHECK_CHOICES: [WizardChoice; 7] = [
    WizardChoice {
        key: "1",
        title: "Inspect latest result",
        detail: "open the most recent coding-run summary",
        command: "agentic-harness inspect --workspace .",
        action: Some(WizardAction::ResultCurrent),
    },
    WizardChoice {
        key: "2",
        title: "Inspect another workspace result",
        detail: "pick a project path and read its latest coding summary",
        command: "prompts for workspace path",
        action: Some(WizardAction::ResultCustom),
    },
    WizardChoice {
        key: "3",
        title: "Open dashboard",
        detail: "show workspace, template, sandbox, logs, and next steps",
        command: "agentic-harness dashboard --workspace .",
        action: Some(WizardAction::DashboardCurrent),
    },
    WizardChoice {
        key: "4",
        title: "Open dashboard for another workspace",
        detail: "pick any project path and inspect its status panels",
        command: "prompts for workspace path",
        action: Some(WizardAction::DashboardCustom),
    },
    WizardChoice {
        key: "5",
        title: "Check current workspace",
        detail: "validate files before you run an agent",
        command: "agentic-harness doctor --workspace .",
        action: Some(WizardAction::DoctorCurrent),
    },
    WizardChoice {
        key: "6",
        title: "Check hello-world example",
        detail: "verify the included example is wired correctly",
        command: "agentic-harness doctor --workspace examples/hello-world",
        action: Some(WizardAction::DoctorExample),
    },
    WizardChoice {
        key: "7",
        title: "Check another workspace",
        detail: "pick any project path and get next steps",
        command: "prompts for workspace path",
        action: Some(WizardAction::DoctorCustom),
    },
];

static WIZARD_STEPS: [WizardStep; 6] = [
    WizardStep {
        key: "1",
        title: "Start coding",
        detail: "create or run the coding-agent loop",
        choices: &CODE_CHOICES,
    },
    WizardStep {
        key: "2",
        title: "Create or install template",
        detail: "manage reusable agent templates",
        choices: &TEMPLATE_CHOICES,
    },
    WizardStep {
        key: "3",
        title: "Set up LLM authoring environment",
        detail: "install instructions for template creation",
        choices: &LLM_CHOICES,
    },
    WizardStep {
        key: "4",
        title: "Set up sandbox",
        detail: "choose where code and shell commands run",
        choices: &SANDBOX_CHOICES,
    },
    WizardStep {
        key: "5",
        title: "Run an agent",
        detail: "invoke local agents with JSON payloads",
        choices: &RUN_CHOICES,
    },
    WizardStep {
        key: "6",
        title: "Check setup",
        detail: "validate required files before running",
        choices: &CHECK_CHOICES,
    },
];

fn prompt_wizard(label: &str) -> Result<String, Box<dyn std::error::Error>> {
    print!("{BLUE}›{RESET} {label}: ");
    io::stdout().flush()?;

    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Err("input closed".into());
    }
    Ok(input)
}

fn find_wizard_step(input: &str) -> Option<&'static WizardStep> {
    WIZARD_STEPS.iter().find(|step| step.key == input)
}

fn find_wizard_choice<'a>(step: &'a WizardStep, input: &str) -> Option<&'a WizardChoice> {
    step.choices.iter().find(|choice| choice.key == input)
}

fn wizard_key_hint() -> String {
    match (WIZARD_STEPS.first(), WIZARD_STEPS.last()) {
        (Some(first), Some(last)) => format!("{}-{}", first.key, last.key),
        _ => "a listed number".to_string(),
    }
}

fn run_wizard_step(
    step: &'static WizardStep,
    workspace: &Path,
) -> Result<WizardNav, Box<dyn std::error::Error>> {
    loop {
        println!("{}", styled_wizard_options(step, workspace));
        let input = prompt_wizard("Select option")?;
        match input.trim() {
            "b" | "B" => return Ok(WizardNav::Back),
            "q" | "Q" => {
                println!("{MUTED}bye.{RESET}");
                return Ok(WizardNav::Quit);
            }
            value => {
                let Some(choice) = find_wizard_choice(step, value) else {
                    eprintln!("Unknown option. Enter a listed number, b to go back, or q to quit.");
                    continue;
                };
                wizard_done(step, choice, workspace);
                if let Some(action) = choice.action {
                    match run_wizard_action(action, workspace) {
                        Ok(0) => println!("{GREEN}✓{RESET} {MUTED}done{RESET}\n"),
                        Ok(code) => eprintln!(
                            "[agentic-harness] action exited with status {code}; choose another option or b/q."
                        ),
                        Err(err) => eprintln!(
                            "[agentic-harness] action failed: {err}; choose another option or b/q."
                        ),
                    }
                }
            }
        }
    }
}

fn run_wizard_action(
    action: WizardAction,
    workspace: &Path,
) -> Result<u8, Box<dyn std::error::Error>> {
    match action {
        WizardAction::CodeCurrent => code_command(CodeCommandOptions {
            workspace,
            prompt: None,
            id: "code",
            test_commands: &[],
            apply_patches: &[],
            llm: None,
            no_tests: false,
            commit: None,
            pr: false,
            summary: None,
            summary_json: None,
        }),
        WizardAction::CodeCurrentWithAutoLlm => code_command(CodeCommandOptions {
            workspace,
            prompt: None,
            id: "code",
            test_commands: &[],
            apply_patches: &[],
            llm: Some("auto"),
            no_tests: false,
            commit: None,
            pr: false,
            summary: None,
            summary_json: None,
        }),
        WizardAction::CodeWorkspace => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let prompt = prompt_required("Coding prompt")?;
            code_command(CodeCommandOptions {
                workspace: &workspace,
                prompt: Some(prompt),
                id: "code",
                test_commands: &[],
                apply_patches: &[],
                llm: None,
                no_tests: false,
                commit: None,
                pr: false,
                summary: None,
                summary_json: None,
            })
        }
        WizardAction::ScaffoldCodingProject => {
            let path = prompt_path_default("Project path", Path::new("./my-agent"))?;
            let default_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("my-agent")
                .to_string();
            let name = prompt_default("Cargo package name", &default_name)?;
            scaffold(&path, Some(name), ScaffoldTemplate::Coding)?;
            println!("{MUTED}Next:{RESET} cd {}", path.display());
            Ok(0)
        }
        WizardAction::TemplateList => template_command(TemplateCommands::List {
            workspace: workspace.to_path_buf(),
            verbose: true,
            json: false,
        }),
        WizardAction::TemplateInit => {
            let path = prompt_path_default("Template path", Path::new("./my-template"))?;
            let default_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("my-template")
                .to_string();
            let name = prompt_default("Template name", &default_name)?;
            let agent = prompt_default("Agent name", &name)?;
            let description = prompt_default("Description", "Reusable Agentic Harness template")?;
            template_command(TemplateCommands::Init {
                path,
                name: Some(name),
                agent: Some(agent),
                description,
                version: "0.1.0".to_string(),
            })
        }
        WizardAction::TemplateAuthor => {
            let name = prompt_default("Template name", "my-template")?;
            let env = prompt_default("LLM environment", "codex")?;
            let prompt = prompt_required("Template goal")?;
            let open = prompt_yes_no("Open selected LLM now", false)?;
            template_command(TemplateCommands::Author {
                name,
                workspace: workspace.to_path_buf(),
                env,
                prompt: Some(prompt),
                open,
                json: false,
            })
        }
        WizardAction::TemplateSearch => {
            let query = prompt_default("Search templates", "review")?;
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            template_command(TemplateCommands::Search {
                query,
                workspace,
                json: false,
            })
        }
        WizardAction::TemplateShow => {
            let template = prompt_default("Template name or path", "coding")?;
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            template_command(TemplateCommands::Show {
                template,
                workspace,
                json: false,
            })
        }
        WizardAction::TemplateValidate => {
            let path = prompt_path_default("Template path", Path::new("./template-name"))?;
            template_command(TemplateCommands::Validate { path })
        }
        WizardAction::TemplateExport => {
            let template = prompt_default("Template name or path", "coding")?;
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let output = prompt_path_default("Export path", Path::new("./coding-template"))?;
            template_command(TemplateCommands::Export {
                template,
                workspace,
                output,
            })
        }
        WizardAction::TemplateInstall => {
            let path = prompt_path_default("Template path", Path::new("./template-name"))?;
            let workspace = prompt_path_default("Install into workspace", Path::new("."))?;
            let scope = prompt_template_scope_default(TemplateScope::Workspace)?;
            template_command(TemplateCommands::Install {
                path,
                workspace,
                scope,
                name: None,
            })
        }
        WizardAction::TemplateImport => {
            let path = prompt_path_default("Template path", Path::new("./template-export"))?;
            let workspace = prompt_path_default("Import into workspace", Path::new("."))?;
            let scope = prompt_template_scope_default(TemplateScope::Workspace)?;
            let name = prompt_default("Installed name, blank uses manifest", "")?;
            let name = if name.trim().is_empty() {
                None
            } else {
                Some(name)
            };
            template_command(TemplateCommands::Import {
                path,
                workspace,
                scope,
                name,
            })
        }
        WizardAction::SetupLlm(environment) => {
            setup_llm_command(workspace, environment.slug(), false)
        }
        WizardAction::SetupLlmAuto => setup_llm_command(workspace, "auto", false),
        WizardAction::SetupSandbox(target) => {
            setup_sandbox_command(workspace, target.slug(), None, false)
        }
        WizardAction::SetupSandboxRemote(target) => {
            setup_sandbox_command(workspace, target.slug(), None, true)
        }
        WizardAction::SandboxStatus => sandbox_command(SandboxCommands::Status {
            workspace: workspace.to_path_buf(),
            json: false,
        }),
        WizardAction::SandboxExec => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let command = prompt_default("Command", "pwd")?;
            sandbox_command(SandboxCommands::Exec {
                command,
                workspace,
                json: false,
            })
        }
        WizardAction::SandboxSync => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let source = prompt_path_default("Source", Path::new("./seed"))?;
            let destination =
                prompt_path_default("Sandbox destination", Path::new("./workspace-seed"))?;
            sandbox_command(SandboxCommands::Sync {
                source,
                destination,
                workspace,
            })
        }
        WizardAction::SandboxLogs => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            sandbox_command(SandboxCommands::Logs {
                workspace,
                json: false,
            })
        }
        WizardAction::SandboxCleanup => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let path = prompt_path_default("Sandbox path", Path::new("./workspace-seed"))?;
            let recursive = prompt_yes_no("Recursive", true)?;
            sandbox_command(SandboxCommands::Rm {
                path,
                recursive,
                workspace,
            })
        }
        WizardAction::ResultCurrent => result_command(workspace, false),
        WizardAction::ResultCustom => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            result_command(&workspace, false)
        }
        WizardAction::DashboardCurrent => dashboard_command(workspace, false, false),
        WizardAction::DashboardCustom => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            dashboard_command(&workspace, false, false)
        }
        WizardAction::DoctorCurrent => doctor_command(workspace, false, false),
        WizardAction::DoctorExample => doctor_command(&default_example_workspace(), false, false),
        WizardAction::DoctorCustom => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            doctor_command(&workspace, false, false)
        }
        WizardAction::RunHello => {
            let workspace = prompt_path_default("Workspace", &default_example_workspace())?;
            let id = prompt_default("Request id", "demo")?;
            let payload = prompt_json_default("Payload JSON", r#"{"name":"Ada"}"#)?;
            run_cargo(
                &workspace,
                [
                    "--agentic-harness-run".to_string(),
                    "hello".to_string(),
                    "--id".to_string(),
                    id,
                    "--payload".to_string(),
                    payload,
                ],
                &[],
            )
        }
        WizardAction::RunCustom => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let agent = prompt_default("Agent name", "hello")?;
            let id = prompt_default("Request id", "demo")?;
            let payload = prompt_json_default("Payload JSON", "{}")?;
            run_cargo(
                &workspace,
                [
                    "--agentic-harness-run".to_string(),
                    agent,
                    "--id".to_string(),
                    id,
                    "--payload".to_string(),
                    payload,
                ],
                &[],
            )
        }
        WizardAction::RunWithEnv => {
            let workspace = prompt_path_default("Workspace", Path::new("."))?;
            let agent = prompt_default("Agent name", "hello")?;
            let id = prompt_default("Request id", "demo")?;
            let payload = prompt_json_default("Payload JSON", "{}")?;
            let env_files = prompt_env_files()?;
            run_cargo(
                &workspace,
                [
                    "--agentic-harness-run".to_string(),
                    agent,
                    "--id".to_string(),
                    id,
                    "--payload".to_string(),
                    payload,
                ],
                &env_files,
            )
        }
        WizardAction::SetupHosting => {
            let addr = prompt_default("Local host address", "127.0.0.1:3583")?;
            setup_hosting_command(workspace, &addr)
        }
        WizardAction::HostingStatus => hosting_command(HostingCommands::Status {
            workspace: workspace.to_path_buf(),
            json: false,
        }),
        WizardAction::HostCurrent => host_command(workspace, None, false, &[]),
        WizardAction::HostCurrentDev => host_command(workspace, None, true, &[]),
    }
}

fn default_example_workspace() -> PathBuf {
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("examples/hello-world")
}

fn prompt_default(label: &str, default: &str) -> Result<String, Box<dyn std::error::Error>> {
    print!("{BLUE}›{RESET} {label} [{default}]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Err("input closed".into());
    }
    let value = input.trim();
    if value.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(value.to_string())
    }
}

fn prompt_required(label: &str) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        print!("{BLUE}›{RESET} {label}: ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Err("input closed".into());
        }
        let value = input.trim();
        if !value.is_empty() {
            return Ok(value.to_string());
        }
        eprintln!("{ORANGE}!{RESET} {label} is required.");
    }
}

fn prompt_path_default(label: &str, default: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let default = default.display().to_string();
    Ok(PathBuf::from(prompt_default(label, &default)?))
}

fn prompt_json_default(label: &str, default: &str) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        let value = prompt_default(label, default)?;
        match serde_json::from_str::<serde_json::Value>(&value) {
            Ok(_) => return Ok(value),
            Err(err) => eprintln!("{ORANGE}!{RESET} Invalid JSON: {err}"),
        }
    }
}

fn prompt_env_files() -> Result<Vec<PathBuf>, Box<dyn std::error::Error>> {
    print!("{BLUE}›{RESET} Env files, comma-separated [none]: ");
    io::stdout().flush()?;

    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Err("input closed".into());
    }
    Ok(input
        .trim()
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .collect())
}

fn prompt_yes_no(label: &str, default: bool) -> Result<bool, Box<dyn std::error::Error>> {
    let hint = if default { "Y/n" } else { "y/N" };
    loop {
        print!("{BLUE}›{RESET} {label}? [{hint}]: ");
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            return Err("input closed".into());
        }
        let value = input.trim().to_lowercase();
        if value.is_empty() {
            return Ok(default);
        }
        match value.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => eprintln!("{ORANGE}!{RESET} Enter y or n."),
        }
    }
}

fn prompt_template_scope_default(
    default: TemplateScope,
) -> Result<String, Box<dyn std::error::Error>> {
    loop {
        let value = prompt_default("Template scope (workspace/user/team)", default.slug())?;
        match TemplateScope::parse(&value) {
            Ok(scope) => return Ok(scope.slug().to_string()),
            Err(err) => eprintln!("{ORANGE}!{RESET} {err}"),
        }
    }
}

fn wizard_done(step: &WizardStep, choice: &WizardChoice, workspace: &Path) {
    let command = wizard_choice_command(choice, workspace);
    println!(
        "{PURPLE}◆{RESET}  {DIM}{} / {}{RESET}\n{MUTED}│{RESET}  {}\n{MUTED}└{RESET}  {WHITE}{BOLD}{}{RESET}\n",
        step.title, choice.title, choice.detail, command
    );
}

fn wizard_choice_command(choice: &WizardChoice, workspace: &Path) -> String {
    let workspace_arg = workspace.display();
    match choice.action {
        Some(WizardAction::CodeCurrent) => {
            format!("agentic-harness code --workspace {workspace_arg}")
        }
        Some(WizardAction::CodeCurrentWithAutoLlm) => {
            format!("agentic-harness code --workspace {workspace_arg} --llm auto")
        }
        Some(WizardAction::TemplateList) => {
            format!("agentic-harness templates ls --workspace {workspace_arg} --verbose")
        }
        Some(WizardAction::SetupLlm(environment)) => format!(
            "agentic-harness setup llm --workspace {workspace_arg} --env {}",
            environment.slug()
        ),
        Some(WizardAction::SetupLlmAuto) => {
            format!("agentic-harness setup llm --workspace {workspace_arg} --env auto")
        }
        Some(WizardAction::SetupSandbox(target)) => format!(
            "agentic-harness setup sandbox --workspace {workspace_arg} --target {}",
            target.slug()
        ),
        Some(WizardAction::SandboxStatus) => {
            format!("agentic-harness sandbox status --workspace {workspace_arg}")
        }
        Some(WizardAction::SandboxLogs) => {
            format!("agentic-harness sandbox logs --workspace {workspace_arg}")
        }
        Some(WizardAction::SetupHosting) => {
            format!(
                "agentic-harness setup hosting --workspace {workspace_arg} --addr 127.0.0.1:3583"
            )
        }
        Some(WizardAction::HostingStatus) => {
            format!("agentic-harness hosting status --workspace {workspace_arg}")
        }
        Some(WizardAction::HostCurrent) => {
            format!("agentic-harness host --workspace {workspace_arg}")
        }
        Some(WizardAction::HostCurrentDev) => {
            format!("agentic-harness host --workspace {workspace_arg} --dev")
        }
        Some(WizardAction::ResultCurrent) => {
            format!("agentic-harness inspect --workspace {workspace_arg}")
        }
        Some(WizardAction::DashboardCurrent) => {
            format!("agentic-harness dashboard --workspace {workspace_arg}")
        }
        Some(WizardAction::DoctorCurrent) => {
            format!("agentic-harness doctor --workspace {workspace_arg}")
        }
        _ => choice.command.to_string(),
    }
}

fn styled_wizard_dashboard(workspace: &Path) -> String {
    let targets = [
        ("Local checkout", "default Rust runtime target"),
        ("Local hosting", "loopback HTTP server for agents"),
        ("Vercel Sandbox", "hosted coding sandbox through SessionEnv"),
        ("E2B", "cloud sandbox connector through SessionEnv"),
        ("Template packs", "reusable agent starting points"),
        (
            "LLM authoring",
            "Claude Code, Codex, Cursor, or Wind Server",
        ),
    ];
    let mut out = format!(
        "\n{PURPLE}◇{RESET}  {BOLD}{WHITE}Agentic Harness Wizard{RESET}\n{MUTED}│{RESET}  runtime: {WHITE}native Rust agent engine{RESET}\n{MUTED}│{RESET}\n"
    );
    out.push_str(&render_wizard_status_panel(workspace, true));
    out.push_str(&format!("{MUTED}│{RESET}\n"));
    out.push_str(&format!(
        "{MUTED}│{RESET}  {MUTED}Pick a workflow number first, then pick an option number. Use b/q any time.{RESET}\n{MUTED}│{RESET}\n"
    ));
    out.push_str(&format!(
        "{MUTED}│{RESET}  {BOLD}{ORANGE}Workflows{RESET}\n"
    ));
    for step in WIZARD_STEPS.iter() {
        out.push_str(&format!(
            "{MUTED}│{RESET}  {BLUE}{}{RESET}  {WHITE}{BOLD}{}{RESET}  {MUTED}{} · {} options{RESET}\n",
            step.key,
            step.title,
            step.detail,
            step.choices.len()
        ));
    }
    out.push_str(&format!(
        "{MUTED}│{RESET}\n{MUTED}│{RESET}  {BOLD}{ORANGE}Targets{RESET}\n"
    ));
    for (title, detail) in targets {
        out.push_str(&format!(
            "{MUTED}│{RESET}  {GREEN}●{RESET}  {WHITE}{title}{RESET}  {MUTED}{detail}{RESET}\n"
        ));
    }
    out.push_str(&format!(
        "{MUTED}└{RESET}  {MUTED}q quit · enter a workflow number{RESET}\n"
    ));
    out
}

fn render_wizard_status_panel(workspace: &Path, color: bool) -> String {
    let report = doctor_report(workspace);
    let config = load_sandbox_config(workspace);
    let hosting = hosting_status(workspace);
    let built_in_templates = BUILT_IN_TEMPLATES.len();
    let scoped_templates = [
        TemplateScope::Workspace,
        TemplateScope::User,
        TemplateScope::Team,
    ]
    .into_iter()
    .filter_map(|scope| template_scope_entries(workspace, scope).ok())
    .map(|entries| entries.len())
    .sum::<usize>();
    let log_count = recent_sandbox_logs(workspace, 5).len();
    let readiness = if report.required_ready() {
        if report.has_optional_warnings() {
            "ready with optional warnings"
        } else {
            "ready"
        }
    } else {
        "blocked"
    };
    let logs = if log_count == 0 {
        "none".to_string()
    } else {
        format!(
            "{log_count} recent entr{}",
            if log_count == 1 { "y" } else { "ies" }
        )
    };
    let workspace_arg = workspace.display();
    let next = if report.required_ready() {
        format!("agentic-harness code --workspace {workspace_arg}")
    } else {
        format!("agentic-harness check --workspace {workspace_arg} --plain")
    };

    if color {
        format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}Status panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Workspace{RESET}: {MUTED}{}{RESET}\n\
{MUTED}│{RESET}  {WHITE}Readiness{RESET}: {readiness}\n\
{MUTED}│{RESET}  {WHITE}Sandbox{RESET}: {} {MUTED}(cwd {}){RESET}\n\
{MUTED}│{RESET}  {WHITE}Local hosting{RESET}: {} {MUTED}{}{RESET}\n\
{MUTED}│{RESET}  {WHITE}Templates{RESET}: {built_in_templates} built-in, {scoped_templates} installed\n\
{MUTED}│{RESET}  {WHITE}Recent logs{RESET}: {logs}\n\
{MUTED}│{RESET}  {WHITE}Next step{RESET}: {next}\n",
            report.workspace.display(),
            config.target,
            config.cwd,
            hosting.addr,
            if hosting.servable { "ready" } else { "blocked" }
        )
    } else {
        format!(
            "Status panel:\n  Workspace: {}\n  Readiness: {readiness}\n  Sandbox: {} (cwd {})\n  Local hosting: {} {}\n  Templates: {built_in_templates} built-in, {scoped_templates} installed\n  Recent logs: {logs}\n  Next step: {next}\n",
            report.workspace.display(),
            config.target,
            config.cwd,
            hosting.addr,
            if hosting.servable { "ready" } else { "blocked" }
        )
    }
}

fn styled_wizard_options(step: &WizardStep, workspace: &Path) -> String {
    let mut out = format!(
        "\n{PURPLE}◇{RESET}  {BOLD}{WHITE}{}{RESET}\n{MUTED}│{RESET}  {MUTED}{}{RESET}\n{MUTED}│{RESET}\n",
        step.title, step.detail
    );
    let panel = render_wizard_step_panel(step, workspace, true);
    if !panel.is_empty() {
        out.push_str(&panel);
        out.push_str(&format!("{MUTED}│{RESET}\n"));
    }
    for choice in step.choices {
        let command = wizard_choice_command(choice, workspace);
        out.push_str(&format!(
            "{MUTED}│{RESET}  {BLUE}{}{RESET}  {WHITE}{BOLD}{}{RESET}  {MUTED}{}{RESET}\n",
            choice.key, choice.title, choice.detail
        ));
        out.push_str(&format!("{MUTED}│{RESET}     {MUTED}↳ {command}{RESET}\n"));
    }
    out.push_str(&format!(
        "{MUTED}└{RESET}  {MUTED}b back · q quit · enter an option{RESET}\n"
    ));
    out
}

fn render_wizard_step_panel(step: &WizardStep, workspace: &Path, color: bool) -> String {
    match step.key {
        "1" => render_coding_wizard_panel(workspace, color),
        "2" => render_template_wizard_panel(workspace, color),
        "3" => render_llm_wizard_panel(workspace, color),
        "4" => render_sandbox_wizard_panel(workspace, color),
        "5" => render_run_wizard_panel(workspace, color),
        "6" => render_check_wizard_panel(workspace, color),
        _ => String::new(),
    }
}

fn render_coding_wizard_panel(workspace: &Path, color: bool) -> String {
    let workspace_arg = workspace.display();
    let summary = workspace.join(DEFAULT_CODING_SUMMARY_PATH);
    let latest = if summary.exists() {
        summary.display().to_string()
    } else {
        "none yet".to_string()
    };
    let latest_loop = latest_coding_run_json(workspace)
        .map(|run| latest_coding_loop_lines(&run).join(", "))
        .filter(|line| !line.is_empty())
        .unwrap_or_else(|| "none yet".to_string());
    let tools = llm_tool_status();

    if color {
        format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}Coding panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Workspace{RESET}: {}\n\
{MUTED}│{RESET}  {WHITE}LLM tools{RESET}: {tools}\n\
{MUTED}│{RESET}  {WHITE}Latest result{RESET}: {latest}\n\
{MUTED}│{RESET}  {WHITE}Latest loop{RESET}: {latest_loop}\n\
{MUTED}│{RESET}  {WHITE}Inspect{RESET}: agentic-harness inspect --workspace {workspace_arg}\n",
            workspace.display()
        )
    } else {
        format!(
            "Coding panel:\n  Workspace: {}\n  LLM tools: {tools}\n  Latest result: {latest}\n  Latest loop: {latest_loop}\n  Inspect: agentic-harness inspect --workspace {workspace_arg}\n",
            workspace.display()
        )
    }
}

fn render_template_wizard_panel(workspace: &Path, color: bool) -> String {
    let workspace_arg = workspace.display();
    let built_in_templates = BUILT_IN_TEMPLATES.len();
    let scoped_templates = [
        TemplateScope::Workspace,
        TemplateScope::User,
        TemplateScope::Team,
    ]
    .into_iter()
    .filter_map(|scope| template_scope_entries(workspace, scope).ok())
    .map(|entries| entries.len())
    .sum::<usize>();
    let preview_templates = [
        ScaffoldTemplate::Coding,
        ScaffoldTemplate::CodeReview,
        ScaffoldTemplate::TestFixer,
    ];

    if color {
        let mut out = format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}Template panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Templates{RESET}: {built_in_templates} built-in, {scoped_templates} installed\n\
{MUTED}│{RESET}  {WHITE}Library JSON{RESET}: agentic-harness templates ls --workspace {workspace_arg} --json\n\
{MUTED}│{RESET}  {WHITE}Preview JSON{RESET}: agentic-harness tpl preview coding --workspace {workspace_arg} --json\n"
        );
        out.push_str(&format!("{MUTED}│{RESET}  {WHITE}Preview picks{RESET}:\n"));
        for template in preview_templates {
            out.push_str(&format!(
                "{MUTED}│{RESET}    {BLUE}{}{RESET}: {MUTED}{}{RESET}\n",
                template.name(),
                template.description()
            ));
        }
        out.push_str(&format!(
            "{MUTED}│{RESET}  {WHITE}Next scaffold{RESET}: agentic-harness new ./my-agent --template coding\n"
        ));
        out
    } else {
        let mut out = format!(
            "Template panel:\n  Templates: {built_in_templates} built-in, {scoped_templates} installed\n  Library JSON: agentic-harness templates ls --workspace {workspace_arg} --json\n  Preview JSON: agentic-harness tpl preview coding --workspace {workspace_arg} --json\n"
        );
        out.push_str("  Preview picks:\n");
        for template in preview_templates {
            out.push_str(&format!(
                "    {}: {}\n",
                template.name(),
                template.description()
            ));
        }
        out.push_str("  Next scaffold: agentic-harness new ./my-agent --template coding\n");
        out
    }
}

fn render_llm_wizard_panel(workspace: &Path, color: bool) -> String {
    let workspace_arg = workspace.display();
    let authoring = if llm_authoring_installed(workspace) {
        "installed"
    } else {
        "not installed"
    };
    let detected = LlmAuthoringEnvironment::detect()
        .map(|environment| environment.label().to_string())
        .unwrap_or_else(|| "none detected; falls back to Codex".to_string());
    let tools = llm_tool_status();

    if color {
        format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}LLM panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Authoring context{RESET}: {authoring}\n\
{MUTED}│{RESET}  {WHITE}Auto detection{RESET}: {detected}\n\
{MUTED}│{RESET}  {WHITE}LLM tools{RESET}: {tools}\n\
{MUTED}│{RESET}  {WHITE}Setup JSON{RESET}: agentic-harness setup llm --workspace {workspace_arg} --env auto\n"
        )
    } else {
        format!(
            "LLM panel:\n  Authoring context: {authoring}\n  Auto detection: {detected}\n  LLM tools: {tools}\n  Setup JSON: agentic-harness setup llm --workspace {workspace_arg} --env auto\n"
        )
    }
}

fn render_sandbox_wizard_panel(workspace: &Path, color: bool) -> String {
    let workspace_arg = workspace.display();
    let config = load_sandbox_config(workspace);
    let smoke = run_sandbox_smoke(workspace, &config);
    let log_count = recent_sandbox_logs(workspace, 5).len();
    let logs = if log_count == 0 {
        "none".to_string()
    } else {
        format!(
            "{log_count} recent entr{}",
            if log_count == 1 { "y" } else { "ies" }
        )
    };
    let latest_log = recent_sandbox_logs(workspace, 1)
        .into_iter()
        .next()
        .unwrap_or_else(|| "none".to_string());
    let smoke_status = if smoke.ok { "ok" } else { "failed" };

    if color {
        format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}Sandbox panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Target{RESET}: {} {MUTED}(cwd {}){RESET}\n\
{MUTED}│{RESET}  {WHITE}Smoke{RESET}: {smoke_status} {MUTED}- {}{RESET}\n\
{MUTED}│{RESET}  {WHITE}Recent logs{RESET}: {logs}\n\
{MUTED}│{RESET}  {WHITE}Recent log{RESET}: {latest_log}\n\
{MUTED}│{RESET}  {WHITE}Status JSON{RESET}: agentic-harness sandbox status --workspace {workspace_arg} --json\n\
{MUTED}│{RESET}  {WHITE}Logs JSON{RESET}: agentic-harness sandbox logs --workspace {workspace_arg} --json\n",
            config.target, config.cwd, smoke.detail
        )
    } else {
        format!(
            "Sandbox panel:\n  Target: {} (cwd {})\n  Smoke: {smoke_status} - {}\n  Recent logs: {logs}\n  Recent log: {latest_log}\n  Status JSON: agentic-harness sandbox status --workspace {workspace_arg} --json\n  Logs JSON: agentic-harness sandbox logs --workspace {workspace_arg} --json\n",
            config.target, config.cwd, smoke.detail
        )
    }
}

fn render_run_wizard_panel(workspace: &Path, color: bool) -> String {
    let example = default_example_workspace();
    let example_display = example.display();
    let hosting = hosting_status(workspace);
    let workspace_arg = workspace.display();

    if color {
        format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}Run panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Example workspace{RESET}: {example_display}\n\
{MUTED}│{RESET}  {WHITE}Manifest{RESET}: agentic-harness manifest --workspace {example_display}\n\
{MUTED}│{RESET}  {WHITE}Payload check{RESET}: JSON is validated before run\n\
{MUTED}│{RESET}  {WHITE}Local hosting{RESET}: {} {MUTED}{}{RESET}\n\
{MUTED}│{RESET}  {WHITE}Host command{RESET}: agentic-harness host --workspace {workspace_arg}\n\
{MUTED}│{RESET}  {WHITE}Host status{RESET}: agentic-harness hosting status --workspace {workspace_arg}\n",
            hosting.addr,
            if hosting.servable { "ready" } else { "blocked" }
        )
    } else {
        format!(
            "Run panel:\n  Example workspace: {example_display}\n  Manifest: agentic-harness manifest --workspace {example_display}\n  Payload check: JSON is validated before run\n  Local hosting: {} {}\n  Host command: agentic-harness host --workspace {workspace_arg}\n  Host status: agentic-harness hosting status --workspace {workspace_arg}\n",
            hosting.addr,
            if hosting.servable { "ready" } else { "blocked" }
        )
    }
}

fn render_check_wizard_panel(workspace: &Path, color: bool) -> String {
    let workspace_arg = workspace.display();
    let report = doctor_report(workspace);
    let readiness = if report.required_ready() {
        "ready"
    } else {
        "blocked"
    };
    let next_fix = report
        .checks
        .iter()
        .find(|check| !check.ok)
        .map(|check| check.fix)
        .unwrap_or("agentic-harness code");

    if color {
        format!(
            "{MUTED}│{RESET}  {BOLD}{ORANGE}Check panel{RESET}\n\
{MUTED}│{RESET}  {WHITE}Readiness{RESET}: {readiness}\n\
{MUTED}│{RESET}  {WHITE}Next fix{RESET}: {next_fix}\n\
{MUTED}│{RESET}  {WHITE}Doctor JSON{RESET}: agentic-harness doctor --workspace {workspace_arg} --json\n\
{MUTED}│{RESET}  {WHITE}Dashboard JSON{RESET}: agentic-harness dashboard --workspace {workspace_arg} --json\n"
        )
    } else {
        format!(
            "Check panel:\n  Readiness: {readiness}\n  Next fix: {next_fix}\n  Doctor JSON: agentic-harness doctor --workspace {workspace_arg} --json\n  Dashboard JSON: agentic-harness dashboard --workspace {workspace_arg} --json\n"
        )
    }
}

fn llm_tool_status() -> String {
    llm_tool_status_from(&llm_tool_availability())
}

#[derive(Debug)]
struct LlmToolAvailability {
    environment: LlmAuthoringEnvironment,
    label: &'static str,
    command: &'static str,
    found: bool,
    ready: bool,
    detail: String,
}

fn llm_tool_availability() -> Vec<LlmToolAvailability> {
    [
        LlmAuthoringEnvironment::ClaudeCode,
        LlmAuthoringEnvironment::Codex,
        LlmAuthoringEnvironment::Cursor,
        LlmAuthoringEnvironment::WindServer,
    ]
    .into_iter()
    .map(|environment| {
        let command = llm_authoring_command_name(environment);
        let found = command_available(command);
        let (ready, detail) = llm_tool_readiness(environment, command, found);
        LlmToolAvailability {
            environment,
            label: environment.label(),
            command,
            found,
            ready,
            detail,
        }
    })
    .collect()
}

fn llm_tool_status_from(tools: &[LlmToolAvailability]) -> String {
    tools
        .iter()
        .map(|tool| format!("{} {}", tool.label, tool.detail))
        .collect::<Vec<_>>()
        .join(", ")
}

fn llm_tool_json_entries(tools: &[LlmToolAvailability]) -> Vec<serde_json::Value> {
    tools
        .iter()
        .map(|tool| {
            serde_json::json!({
                "environment": tool.environment.slug(),
                "label": tool.label,
                "command": tool.command,
                "found": tool.found,
                "ready": tool.ready,
                "detail": tool.detail,
            })
        })
        .collect()
}

fn llm_tool_readiness(
    environment: LlmAuthoringEnvironment,
    command: &str,
    found: bool,
) -> (bool, String) {
    if !found {
        return (false, "missing".to_string());
    }
    match environment {
        LlmAuthoringEnvironment::ClaudeCode => {
            llm_probe_json_login(command, &["auth", "status"], "loggedIn", "login required")
        }
        LlmAuthoringEnvironment::Codex => llm_probe_text_login(command, &["login", "status"]),
        LlmAuthoringEnvironment::Cursor => llm_probe_json_login(
            command,
            &["agent", "status", "--format", "json"],
            "authenticated",
            "login or keychain access required",
        ),
        LlmAuthoringEnvironment::WindServer => (true, "ready".to_string()),
    }
}

fn llm_probe_json_login(
    command: &str,
    args: &[&str],
    key: &str,
    missing_detail: &str,
) -> (bool, String) {
    match run_status_probe(command, args) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let authenticated = serde_json::from_str::<serde_json::Value>(&stdout)
                .ok()
                .and_then(|value| value.get(key).and_then(serde_json::Value::as_bool))
                .unwrap_or(false);
            if output.status.success() && authenticated {
                (true, "ready".to_string())
            } else if stderr.trim().is_empty() {
                (false, missing_detail.to_string())
            } else {
                (
                    false,
                    format!("{missing_detail}: {}", trim_inline(stderr.trim(), 120)),
                )
            }
        }
        Err(err) => (false, err),
    }
}

fn llm_probe_text_login(command: &str, args: &[&str]) -> (bool, String) {
    match run_status_probe(command, args) {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}\n{stderr}").to_ascii_lowercase();
            if output.status.success() && combined.contains("logged in") {
                (true, "ready".to_string())
            } else if stderr.trim().is_empty() {
                (false, "login required".to_string())
            } else {
                (
                    false,
                    format!("login required: {}", trim_inline(stderr.trim(), 120)),
                )
            }
        }
        Err(err) => (false, err),
    }
}

fn run_status_probe(command: &str, args: &[&str]) -> Result<Output, String> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("status unavailable: {err}"))?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => {
                return child
                    .wait_with_output()
                    .map_err(|err| format!("status unavailable: {err}"));
            }
            Ok(None) if started.elapsed() >= LLM_STATUS_PROBE_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("status timed out".to_string());
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(err) => return Err(format!("status unavailable: {err}")),
        }
    }
}

fn wizard_dashboard(workspace: &Path) -> String {
    let workspace_arg = workspace.display().to_string();
    let mut out = String::from("Agentic Harness Wizard\nruntime: native Rust agent engine\n");
    out.push_str(&render_wizard_status_panel(workspace, false));
    out.push_str(&format!(
        "\nWorkspace commands:\n  agentic-harness code --workspace {}\n  agentic-harness dashboard --workspace {}\n  agentic-harness doctor --workspace {} --plain\n",
        workspace_arg,
        workspace_arg,
        workspace_arg
    ));
    out.push_str(
        r#"
Pick a workflow number first, then pick an option number. Use b to go back or q to quit.
Selection screens show status panels, template previews, sandbox logs, and next commands.

Workflows:
  1. Start coding: create or run the coding-agent loop
     1. Start coding here: agentic-harness code
     2. Start coding with detected LLM: agentic-harness code --llm auto
     3. Start coding in another workspace: prompts for workspace and prompt
     4. Create coding agent: agentic-harness new ./my-agent --template coding
  2. Create or install template: manage reusable agent templates
     1. Create template pack: agentic-harness tpl create ./my-template --name my-template
     2. Create with LLM: agentic-harness template author my-template --env codex --prompt ... --open
     3. List templates: agentic-harness templates ls --verbose
     4. Search templates: agentic-harness templates search review
     5. Preview template: agentic-harness tpl preview coding
     6. Validate template pack: agentic-harness templates check path
     7. Export template pack: agentic-harness template export coding --output ./coding-template
     8. Install template pack: agentic-harness tpl add path --scope workspace
     9. Import template pack: agentic-harness tpl use path --scope workspace
  3. Set up LLM authoring environment: install instructions for template creation
     1. Claude Code: agentic-harness setup llm --env claude-code
     2. Codex: agentic-harness setup llm --env codex
     3. Cursor: agentic-harness setup llm --env cursor
     4. Wind Server: agentic-harness setup llm --env wind-server
     5. Detect automatically: agentic-harness setup llm --env auto
  4. Set up sandbox: choose where code and shell commands run
     1. Local checkout: agentic-harness setup sandbox --target local
     2. Sandbox status: agentic-harness sandbox status
     3. Run sandbox command: agentic-harness sandbox exec command
     4. Sync into sandbox: agentic-harness sandbox sync source destination
     5. Sandbox logs: agentic-harness sandbox logs
     6. Clean sandbox path: agentic-harness sandbox rm path --recursive
     7. Vercel Sandbox: agentic-harness setup sandbox --target vercel --print
     8. Daytona: agentic-harness setup sandbox --target daytona --print
     9. E2B: agentic-harness setup sandbox --target e2b --print
     10. Custom remote sandbox: agentic-harness setup sandbox --target custom --print
  5. Run an agent: invoke local agents with JSON payloads
     1. Run hello locally: agentic-harness run hello --workspace examples/hello-world --id demo --payload '{"name":"Ada"}'
     2. Run from your checkout: prompts for agent, workspace, id, and JSON payload
     3. Run with environment files: prompts for agent, workspace, id, payload, and env files
     4. Configure local hosting: agentic-harness setup hosting --workspace . --addr 127.0.0.1:3583
     5. Hosting status: agentic-harness hosting status --workspace .
     6. Start local host: agentic-harness host --workspace .
     7. Start dev host: agentic-harness host --workspace . --dev
  6. Check setup: validate required files before running
     1. Inspect latest result: agentic-harness inspect --workspace .
     2. Inspect another workspace result: prompts for workspace path
     3. Open dashboard: agentic-harness dashboard --workspace .
     4. Open dashboard for another workspace: prompts for workspace path
     5. Check current workspace: agentic-harness doctor --workspace .
     6. Check hello-world example: agentic-harness doctor --workspace examples/hello-world
     7. Check another workspace: prompts for workspace path

Common commands:
  agentic-harness code
  agentic-harness inspect --workspace .
  agentic-harness tpl create ./my-template --name my-template
  agentic-harness template author my-template --env codex --prompt "Create a coding agent template" --open
  agentic-harness templates ls --verbose
  agentic-harness templates search review
  agentic-harness tpl preview coding
  agentic-harness template export coding --output ./coding-template
  agentic-harness tpl use ./coding-template --scope team
  agentic-harness setup llm
  agentic-harness setup sandbox --target local
  agentic-harness setup sandbox --target e2b --print
  agentic-harness sandbox status
  agentic-harness sandbox exec "pwd"
  agentic-harness sandbox sync ./seed workspace-seed
  agentic-harness sandbox logs
  agentic-harness sandbox rm workspace-seed --recursive
  agentic-harness setup hosting --workspace . --addr 127.0.0.1:3583
  agentic-harness hosting status --workspace .
  agentic-harness host --workspace .
  agentic-harness host --workspace . --dev
  agentic-harness dashboard --workspace .
  agentic-harness doctor --workspace .
  agentic-harness run hello --workspace examples/hello-world --id demo --payload '{"name":"Ada"}'

Targets:
  - Local checkout: default Rust runtime target
  - Local hosting: loopback HTTP server for agents
  - Vercel Sandbox: hosted coding sandbox target through SessionEnv
  - E2B: cloud sandbox target through SessionEnv
  - Template packs: reusable agent starting points
  - LLM authoring: Claude Code, Codex, Cursor, or Wind Server
"#,
    );
    out = out
        .replace(
            "Start coding here: agentic-harness code",
            &format!("Start coding here: agentic-harness code --workspace {workspace_arg}"),
        )
        .replace(
            "Start coding with detected LLM: agentic-harness code --llm auto",
            &format!(
                "Start coding with detected LLM: agentic-harness code --workspace {workspace_arg} --llm auto"
            ),
        )
        .replace(
            "List templates: agentic-harness templates ls --verbose",
            &format!("List templates: agentic-harness templates ls --workspace {workspace_arg} --verbose"),
        )
        .replace(
            "Sandbox status: agentic-harness sandbox status",
            &format!("Sandbox status: agentic-harness sandbox status --workspace {workspace_arg}"),
        )
        .replace(
            "Sandbox logs: agentic-harness sandbox logs",
            &format!("Sandbox logs: agentic-harness sandbox logs --workspace {workspace_arg}"),
        )
        .replace(
            "Configure local hosting: agentic-harness setup hosting --workspace . --addr 127.0.0.1:3583",
            &format!("Configure local hosting: agentic-harness setup hosting --workspace {workspace_arg} --addr 127.0.0.1:3583"),
        )
        .replace(
            "Hosting status: agentic-harness hosting status --workspace .",
            &format!("Hosting status: agentic-harness hosting status --workspace {workspace_arg}"),
        )
        .replace(
            "Start local host: agentic-harness host --workspace .",
            &format!("Start local host: agentic-harness host --workspace {workspace_arg}"),
        )
        .replace(
            "Start dev host: agentic-harness host --workspace . --dev",
            &format!("Start dev host: agentic-harness host --workspace {workspace_arg} --dev"),
        )
        .replace(
            "Inspect latest result: agentic-harness inspect --workspace .",
            &format!("Inspect latest result: agentic-harness inspect --workspace {workspace_arg}"),
        )
        .replace(
            "Open dashboard: agentic-harness dashboard --workspace .",
            &format!("Open dashboard: agentic-harness dashboard --workspace {workspace_arg}"),
        )
        .replace(
            "Check current workspace: agentic-harness doctor --workspace .",
            &format!("Check current workspace: agentic-harness doctor --workspace {workspace_arg}"),
        );
    out
}

fn run_cargo<I>(
    workspace: &Path,
    runtime_args: I,
    env_files: &[PathBuf],
) -> Result<u8, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let output = run_cargo_capture(workspace, runtime_args, env_files)?;
    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;
    Ok(output.status.code().unwrap_or(1) as u8)
}

fn run_cargo_capture<I>(
    workspace: &Path,
    runtime_args: I,
    env_files: &[PathBuf],
) -> Result<Output, Box<dyn std::error::Error>>
where
    I: IntoIterator<Item = String>,
{
    let workspace = workspace.canonicalize()?;
    let manifest = workspace.join("Cargo.toml");
    if !manifest.exists() {
        return Err(format!("No Cargo.toml found at {}", manifest.display()).into());
    }

    let env = parse_env_files(env_files)?;
    let mut command = Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--")
        .args(runtime_args)
        .current_dir(&workspace)
        .envs(env);

    Ok(command.output()?)
}

fn run_dev_server(
    workspace: &Path,
    port: u16,
    env_files: &[PathBuf],
) -> Result<u8, Box<dyn std::error::Error>> {
    let workspace = workspace.canonicalize()?;
    let manifest = workspace.join("Cargo.toml");
    if !manifest.exists() {
        return Err(format!("No Cargo.toml found at {}", manifest.display()).into());
    }

    let env = parse_env_files(env_files)?;
    let addr = format!("127.0.0.1:{port}");
    let mut snapshot = WatchSnapshot::read(&workspace)?;

    eprintln!("[agentic-harness] Watching {}", workspace.display());
    loop {
        eprintln!("[agentic-harness] Starting dev server at http://{addr}");
        let mut child = spawn_dev_child(&workspace, &manifest, &addr, &env)?;

        loop {
            thread::sleep(DEV_WATCH_INTERVAL);

            if let Some(status) = child.try_wait()? {
                let code = status.code().unwrap_or(1) as u8;
                if code == 0 {
                    return Ok(0);
                }
                eprintln!(
                    "[agentic-harness] Dev server exited with status {code}; waiting for a file change before restarting."
                );
                snapshot = wait_for_watch_change(&workspace, snapshot)?;
                break;
            }

            let next = WatchSnapshot::read(&workspace)?;
            if snapshot.has_changed(&next) {
                eprintln!("[agentic-harness] Change detected; restarting dev server.");
                stop_child(&mut child)?;
                snapshot = next;
                break;
            }
        }
    }
}

fn spawn_dev_child(
    workspace: &Path,
    manifest: &Path,
    addr: &str,
    env: &[(String, String)],
) -> Result<Child, Box<dyn std::error::Error>> {
    Ok(Command::new("cargo")
        .arg("run")
        .arg("--manifest-path")
        .arg(manifest)
        .arg("--")
        .arg("--agentic-harness-serve")
        .arg("--addr")
        .arg(addr)
        .current_dir(workspace)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?)
}

fn wait_for_watch_change(
    workspace: &Path,
    mut snapshot: WatchSnapshot,
) -> Result<WatchSnapshot, Box<dyn std::error::Error>> {
    loop {
        thread::sleep(DEV_WATCH_INTERVAL);
        let next = WatchSnapshot::read(workspace)?;
        if snapshot.has_changed(&next) {
            eprintln!("[agentic-harness] Change detected; restarting dev server.");
            return Ok(next);
        }
        snapshot = next;
    }
}

fn stop_child(child: &mut Child) -> Result<(), Box<dyn std::error::Error>> {
    if child.try_wait()?.is_none() {
        let _ = child.kill();
    }
    let _ = child.wait();
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchSnapshot {
    files: BTreeMap<PathBuf, WatchFingerprint>,
}

impl WatchSnapshot {
    fn read(root: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        let root = root.canonicalize()?;
        let mut files = BTreeMap::new();
        collect_watch_entries(&root, &root, &mut files)?;
        Ok(Self { files })
    }

    fn has_changed(&self, next: &Self) -> bool {
        self.files != next.files
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WatchFingerprint {
    len: u64,
    modified_unix_nanos: Option<u128>,
}

fn collect_watch_entries(
    root: &Path,
    path: &Path,
    files: &mut BTreeMap<PathBuf, WatchFingerprint>,
) -> Result<(), Box<dyn std::error::Error>> {
    if path != root && is_ignored_watch_dir(path) {
        return Ok(());
    }

    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_watch_entries(root, &path, files)?;
        } else if file_type.is_file() {
            let metadata = entry.metadata()?;
            let relative = path.strip_prefix(root)?.to_path_buf();
            files.insert(
                relative,
                WatchFingerprint {
                    len: metadata.len(),
                    modified_unix_nanos: metadata
                        .modified()
                        .ok()
                        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                        .map(|duration| duration.as_nanos()),
                },
            );
        }
    }
    Ok(())
}

fn is_ignored_watch_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | "target" | "dist" | "build" | "node_modules" | ".next")
    )
}

fn build_native(
    workspace: &Path,
    output: Option<&Path>,
    env_files: &[PathBuf],
    target: BuildTarget,
) -> Result<u8, Box<dyn std::error::Error>> {
    let workspace = workspace.canonicalize()?;
    let output = resolve_output_dir(output)?;
    let manifest = workspace.join("Cargo.toml");
    if !manifest.exists() {
        return Err(format!("No Cargo.toml found at {}", manifest.display()).into());
    }

    let env = parse_env_files(env_files)?;
    let metadata = cargo_metadata(&manifest)?;
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages.iter().find(|package| {
                package["manifest_path"]
                    .as_str()
                    .map(|path| Path::new(path) == manifest.as_path())
                    .unwrap_or(false)
            })
        })
        .ok_or("cargo metadata did not include the requested package")?;
    let package_name = package["name"]
        .as_str()
        .ok_or("cargo metadata did not include a package name")?;
    let binary_name = package["targets"]
        .as_array()
        .and_then(|targets| {
            targets.iter().find(|target| {
                target["kind"]
                    .as_array()
                    .map(|kinds| kinds.iter().any(|kind| kind.as_str() == Some("bin")))
                    .unwrap_or(false)
            })
        })
        .and_then(|target| target["name"].as_str())
        .unwrap_or(package_name);
    let target_dir = metadata["target_directory"]
        .as_str()
        .ok_or("cargo metadata did not include target_directory")?;

    eprintln!(
        "[agentic-harness] Building native Rust workspace: {}",
        workspace.display()
    );
    eprintln!("[agentic-harness] Output: {}/dist", output.display());
    eprintln!("[agentic-harness] Target: {}", target.label());

    let mut build = Command::new("cargo");
    build
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(&manifest)
        .current_dir(&workspace)
        .envs(env.clone());
    let build_output = build.output()?;
    io::stdout().write_all(&build_output.stdout)?;
    io::stderr().write_all(&build_output.stderr)?;
    if !build_output.status.success() {
        return Ok(build_output.status.code().unwrap_or(1) as u8);
    }

    let built_binary = Path::new(target_dir)
        .join("release")
        .join(format!("{binary_name}{}", std::env::consts::EXE_SUFFIX));
    if !built_binary.exists() {
        return Err(format!("Built binary not found at {}", built_binary.display()).into());
    }

    let dist = output.join("dist");
    fs::create_dir_all(&dist)?;
    let dist_binary = dist.join(format!(
        "agentic-harness-agent{}",
        std::env::consts::EXE_SUFFIX
    ));
    fs::copy(&built_binary, &dist_binary)?;

    let manifest_output = Command::new(&built_binary)
        .arg("--agentic-harness-manifest")
        .current_dir(&workspace)
        .envs(env)
        .output()?;
    io::stderr().write_all(&manifest_output.stderr)?;
    if !manifest_output.status.success() {
        return Ok(manifest_output.status.code().unwrap_or(1) as u8);
    }
    let manifest_json: serde_json::Value = serde_json::from_slice(&manifest_output.stdout)?;
    fs::write(
        dist.join("manifest.json"),
        serde_json::to_string_pretty(&manifest_json)?,
    )?;

    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("manifest.json").display()
    );
    eprintln!("[agentic-harness] Built: {}", dist_binary.display());
    eprintln!(
        "[agentic-harness] Build complete. Output: {}",
        dist.display()
    );
    Ok(0)
}

fn build_cloudflare(
    workspace: &Path,
    output: Option<&Path>,
    worker_app: Option<&Path>,
    worker_wasm: Option<&Path>,
    worker_wasm_crate: Option<&Path>,
    env_files: &[PathBuf],
) -> Result<u8, Box<dyn std::error::Error>> {
    if worker_wasm.is_some() && worker_wasm_crate.is_some() {
        return Err("--worker-wasm and --worker-wasm-crate cannot be used together".into());
    }

    let workspace = workspace.canonicalize()?;
    let output = resolve_output_dir(output)?;
    let manifest = workspace.join("Cargo.toml");
    if !manifest.exists() {
        return Err(format!("No Cargo.toml found at {}", manifest.display()).into());
    }

    let env = parse_env_files(env_files)?;
    let metadata = cargo_metadata(&manifest)?;
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages.iter().find(|package| {
                package["manifest_path"]
                    .as_str()
                    .map(|path| Path::new(path) == manifest.as_path())
                    .unwrap_or(false)
            })
        })
        .ok_or("cargo metadata did not include the requested package")?;
    let package_name = package["name"]
        .as_str()
        .ok_or("cargo metadata did not include a package name")?;

    eprintln!(
        "[agentic-harness] Building Cloudflare Worker boundary artifacts: {}",
        workspace.display()
    );
    eprintln!("[agentic-harness] Output: {}/dist", output.display());
    eprintln!("[agentic-harness] Target: cloudflare worker boundary");

    let manifest_output = Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--")
        .arg("--agentic-harness-cloudflare-manifest")
        .current_dir(&workspace)
        .envs(env.clone())
        .output()?;
    io::stderr().write_all(&manifest_output.stderr)?;
    if !manifest_output.status.success() {
        return Ok(manifest_output.status.code().unwrap_or(1) as u8);
    }

    let worker_manifest: agentic_harness::CloudflareWorkerManifest =
        serde_json::from_slice(&manifest_output.stdout)?;
    let dist = output.join("dist");
    fs::create_dir_all(&dist)?;
    if let Some(worker_wasm_crate) = worker_wasm_crate {
        if let Some(code) = build_worker_wasm_crate(worker_wasm_crate, &dist, &env)? {
            return Ok(code);
        }
    }
    fs::write(
        dist.join("cloudflare-manifest.json"),
        serde_json::to_string_pretty(&worker_manifest)?,
    )?;
    fs::write(
        dist.join("wrangler.jsonc"),
        serde_json::to_string_pretty(&worker_manifest.wrangler_config(package_name, "_entry.js"))?,
    )?;
    fs::write(
        dist.join("_entry.js"),
        worker_manifest.worker_entrypoint("./agentic_harness_worker.js"),
    )?;
    fs::write(
        dist.join("agentic_harness_worker.js"),
        cloudflare_worker_runtime_module(),
    )?;
    let app_adapter = dist.join("agentic_harness_app.js");
    if let Some(worker_app) = worker_app {
        fs::copy(worker_app, &app_adapter)?;
    } else if worker_wasm.is_some() || worker_wasm_crate.is_some() {
        fs::write(&app_adapter, cloudflare_default_wasm_app_adapter())?;
    } else {
        fs::write(&app_adapter, cloudflare_app_adapter_stub())?;
    }
    if let Some(worker_wasm) = worker_wasm {
        fs::copy(worker_wasm, dist.join("agentic_harness_app.wasm"))?;
    }
    fs::write(
        dist.join("agentic_harness_app.d.ts"),
        cloudflare_app_adapter_types(),
    )?;

    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("_entry.js").display()
    );
    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("agentic_harness_worker.js").display()
    );
    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("agentic_harness_app.js").display()
    );
    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("agentic_harness_app.d.ts").display()
    );
    if worker_wasm.is_some() || worker_wasm_crate.is_some() {
        eprintln!(
            "[agentic-harness] Generated: {}",
            dist.join("agentic_harness_app.wasm").display()
        );
    }
    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("wrangler.jsonc").display()
    );
    eprintln!(
        "[agentic-harness] Generated: {}",
        dist.join("cloudflare-manifest.json").display()
    );
    eprintln!(
        "[agentic-harness] Cloudflare build currently emits non-proxy Worker boundary artifacts; handler execution still requires a Worker-compatible app adapter."
    );
    Ok(0)
}

fn resolve_output_dir(output: Option<&Path>) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let path = output
        .map(Path::to_path_buf)
        .unwrap_or(std::env::current_dir()?);
    Ok(path.canonicalize().unwrap_or(path))
}

fn build_worker_wasm_crate(
    crate_path: &Path,
    dist: &Path,
    env: &[(String, String)],
) -> Result<Option<u8>, Box<dyn std::error::Error>> {
    let manifest = if crate_path.is_dir() {
        crate_path.join("Cargo.toml")
    } else {
        crate_path.to_path_buf()
    };
    if !manifest.exists() {
        return Err(format!("No Cargo.toml found at {}", manifest.display()).into());
    }

    let metadata = cargo_metadata(&manifest)?;
    let package = metadata["packages"]
        .as_array()
        .and_then(|packages| {
            packages.iter().find(|package| {
                package["manifest_path"]
                    .as_str()
                    .map(|path| Path::new(path) == manifest.as_path())
                    .unwrap_or(false)
            })
        })
        .ok_or("cargo metadata did not include the requested worker WASM package")?;
    let package_name = package["name"]
        .as_str()
        .ok_or("cargo metadata did not include a package name")?;
    let target_name = package["targets"]
        .as_array()
        .and_then(|targets| {
            targets.iter().find(|target| {
                target["kind"]
                    .as_array()
                    .map(|kinds| {
                        kinds
                            .iter()
                            .any(|kind| matches!(kind.as_str(), Some("cdylib" | "lib" | "bin")))
                    })
                    .unwrap_or(false)
            })
        })
        .and_then(|target| target["name"].as_str())
        .unwrap_or(package_name);
    let artifact_name = format!("{}.wasm", target_name.replace('-', "_"));
    let target_dir = dist.join(".agentic-harness-wasm-target");

    eprintln!(
        "[agentic-harness] Building Worker WASM crate: {}",
        manifest.display()
    );
    let output = Command::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--target")
        .arg("wasm32-unknown-unknown")
        .arg("--no-default-features")
        .arg("--manifest-path")
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target_dir)
        .current_dir(manifest.parent().unwrap_or_else(|| Path::new(".")))
        .envs(env.iter().map(|(key, value)| (key, value)))
        .output()?;
    io::stdout().write_all(&output.stdout)?;
    io::stderr().write_all(&output.stderr)?;
    if !output.status.success() {
        return Ok(Some(output.status.code().unwrap_or(1) as u8));
    }

    let artifact = target_dir
        .join("wasm32-unknown-unknown")
        .join("release")
        .join(artifact_name);
    if !artifact.exists() {
        return Err(format!("Worker WASM artifact not found at {}", artifact.display()).into());
    }
    fs::copy(artifact, dist.join("agentic_harness_app.wasm"))?;
    Ok(None)
}

fn cloudflare_worker_runtime_module() -> &'static str {
    r#"// Auto-generated by Agentic Harness Cloudflare build.
import { initAgenticHarnessApp, invokeAgenticHarnessAgent } from './agentic_harness_app.js';

export default async function initWorker() {
  if (typeof initAgenticHarnessApp === 'function') {
    await initAgenticHarnessApp();
  }
}

const fallbackSessions = new Map();

function jsonResponse(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function errorEnvelope(error) {
  if (error && typeof error === 'object' && error.error) {
    return error;
  }
  if (error && typeof error === 'object' && error.type && error.message) {
    return { error: { type: String(error.type), message: String(error.message) } };
  }
  return {
    error: {
      type: 'handler_error',
      message: error instanceof Error ? error.message : String(error),
    },
  };
}

function errorStatus(error) {
  if (error && typeof error === 'object' && Number.isInteger(error.status)) {
    return error.status;
  }
  return 500;
}

async function parseJsonBody(request) {
  const text = await request.text();
  if (text.trim() === '') {
    return {};
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    throw {
      status: 400,
      type: 'invalid_json',
      message: error instanceof Error ? error.message : String(error),
    };
  }
}

function sseFrame(event, data, id) {
  return `event: ${event}\nid: ${id}\ndata: ${JSON.stringify(data)}\n\n`;
}

function createSessionStore(state) {
  const sql = state?.storage?.sql;
  if (sql && typeof sql.exec === 'function') {
    sql.exec(
      'CREATE TABLE IF NOT EXISTS agentic_harness_sessions (id TEXT PRIMARY KEY, data TEXT NOT NULL, updated_at INTEGER NOT NULL)'
    );
    return {
      async load(key) {
        const rows = sql.exec('SELECT data FROM agentic_harness_sessions WHERE id = ?', key).toArray();
        if (rows.length === 0) {
          return null;
        }
        return JSON.parse(rows[0].data);
      },
      async save(key, data) {
        sql.exec(
          'INSERT OR REPLACE INTO agentic_harness_sessions (id, data, updated_at) VALUES (?, ?, ?)',
          key,
          JSON.stringify(data),
          Date.now()
        );
      },
      async delete(key) {
        sql.exec('DELETE FROM agentic_harness_sessions WHERE id = ?', key);
      },
    };
  }

  return {
    async load(key) {
      return fallbackSessions.get(key) ?? null;
    },
    async save(key, data) {
      fallbackSessions.set(key, data);
    },
    async delete(key) {
      fallbackSessions.delete(key);
    },
  };
}

function requestRouteParts(request) {
  const url = new URL(request.url);
  const match = url.pathname.match(/^\/agents\/([^/]+)\/([^/]+)\/?$/);
  return {
    agentName: match ? decodeURIComponent(match[1]) : '',
    id: match ? decodeURIComponent(match[2]) : '',
  };
}

function createRuntimeContext(request, state, env, agentName, payload, emit) {
  const route = requestRouteParts(request);
  return {
    request,
    state,
    env,
    agentName,
    id: route.id,
    payload,
    sessionStore: createSessionStore(state),
    emit,
  };
}

export async function handleAgenticHarnessDurableObject(request, state, env, agentName) {
  try {
    const payload = await parseJsonBody(request);
    const accept = request.headers.get('accept') || '';
    const isWebhook = request.headers.get('x-webhook') === 'true';
    const isSse = accept.includes('text/event-stream') && !isWebhook;

    if (isWebhook) {
      const requestId = crypto.randomUUID();
      const context = createRuntimeContext(request, state, env, agentName, payload, () => {});
      Promise.resolve()
        .then(() => invokeAgenticHarnessAgent(context))
        .catch((error) => console.error('[agentic-harness] Cloudflare webhook handler error', error));
      return jsonResponse({ status: 'accepted', requestId }, 202);
    }

    if (isSse) {
      const { readable, writable } = new TransformStream();
      const writer = writable.getWriter();
      const encoder = new TextEncoder();
      let eventId = 0;
      const writeSse = async (event, data) => {
        await writer.write(encoder.encode(sseFrame(event, data, eventId++)));
      };
      const context = createRuntimeContext(request, state, env, agentName, payload, (event) => {
        writeSse(event.event ?? 'message', event.data ?? event).catch(() => {});
      });

      Promise.resolve()
        .then(async () => {
          await writeSse('agent_start', { agent: agentName, id: context.id });
          const result = await invokeAgenticHarnessAgent(context);
          await writeSse('result', result ?? null);
          await writeSse('idle', {});
        })
        .catch(async (error) => {
          await writeSse('error', errorEnvelope(error));
          await writeSse('idle', {});
        })
        .finally(async () => {
          await writer.close();
        });

      return new Response(readable, {
        headers: {
          'content-type': 'text/event-stream',
          'cache-control': 'no-cache',
          connection: 'keep-alive',
        },
      });
    }

    const context = createRuntimeContext(request, state, env, agentName, payload, () => {});
    const result = await invokeAgenticHarnessAgent(context);
    return jsonResponse({ result: result ?? null });
  } catch (error) {
    return jsonResponse(errorEnvelope(error), errorStatus(error));
  }
}
"#
}

fn cloudflare_app_adapter_stub() -> &'static str {
    r#"// Auto-generated by Agentic Harness Cloudflare build.

export async function initAgenticHarnessApp() {
  return undefined;
}

export async function invokeAgenticHarnessAgent(context) {
  throw {
    status: 501,
    type: 'handler_not_linked',
    message: `No Worker-compatible Agentic Harness app adapter is linked for "${context.agentName}". Replace agentic_harness_app.js with the Rust/WASM adapter for this workspace.`,
  };
}
"#
}

fn cloudflare_default_wasm_app_adapter() -> &'static str {
    r#"// Auto-generated by Agentic Harness Cloudflare build.
import wasmModule from './agentic_harness_app.wasm';

const textEncoder = new TextEncoder();
const textDecoder = new TextDecoder();
let wasmInstance;

export async function initAgenticHarnessApp() {
  if (wasmInstance) {
    return wasmInstance;
  }
  const instance = await WebAssembly.instantiate(wasmModule, {
    env: {
      agentic_harness_emit(_ptr, _len) {
        // Event callbacks are passed through the JSON request/response ABI.
      },
    },
  });
  wasmInstance = instance;
  if (typeof wasmInstance.exports.agentic_harness_init === 'function') {
    wasmInstance.exports.agentic_harness_init();
  }
  return wasmInstance;
}

function requireExport(name) {
  const value = wasmInstance?.exports?.[name];
  if (typeof value !== 'function') {
    throw {
      status: 500,
      type: 'wasm_abi_error',
      message: `Agentic Harness WASM adapter is missing export "${name}".`,
    };
  }
  return value;
}

function writeJson(value) {
  const bytes = textEncoder.encode(JSON.stringify(value));
  const alloc = requireExport('agentic_harness_alloc');
  const ptr = alloc(bytes.length);
  new Uint8Array(wasmInstance.exports.memory.buffer, ptr, bytes.length).set(bytes);
  return { ptr, len: bytes.length };
}

function readJson(ptr, len) {
  const bytes = new Uint8Array(wasmInstance.exports.memory.buffer, ptr, len);
  const text = textDecoder.decode(bytes);
  return text.length === 0 ? null : JSON.parse(text);
}

export async function invokeAgenticHarnessAgent(context) {
  await initAgenticHarnessApp();
  const invoke = requireExport('agentic_harness_invoke');
  const request = writeJson({
    agentName: context.agentName,
    id: context.id,
    payload: context.payload,
  });
  const resultPtr = invoke(request.ptr, request.len);
  const resultLen = Number(wasmInstance.exports.agentic_harness_last_result_len?.() ?? 0);
  const response = readJson(resultPtr, resultLen);
  if (response?.error) {
    const error = response.error.error ?? response.error;
    throw {
      status: response.status ?? 500,
      type: error.type ?? 'handler_error',
      message: error.message ?? 'Agentic Harness WASM adapter returned an error.',
    };
  }
  return response.result ?? null;
}
"#
}

fn cloudflare_app_adapter_types() -> &'static str {
    r#"// Auto-generated by Agentic Harness Cloudflare build.

export interface AgenticHarnessSessionStore {
  load(key: string): Promise<unknown | null>;
  save(key: string, data: unknown): Promise<void>;
  delete(key: string): Promise<void>;
}

export interface AgenticHarnessWorkerEvent {
  event: string;
  data: unknown;
}

export interface AgenticHarnessWorkerContext {
  request: Request;
  state: DurableObjectState;
  env: Record<string, unknown>;
  agentName: string;
  id: string;
  payload: unknown;
  sessionStore: AgenticHarnessSessionStore;
  emit(event: AgenticHarnessWorkerEvent): void;
}

export function initAgenticHarnessApp(): Promise<void> | void;
export function invokeAgenticHarnessAgent(
  context: AgenticHarnessWorkerContext,
): Promise<unknown>;

export interface AgenticHarnessWasmExports extends WebAssembly.Exports {
  memory: WebAssembly.Memory;
  agentic_harness_alloc(len: number): number;
  agentic_harness_invoke(ptr: number, len: number): number;
  agentic_harness_last_result_len(): number;
  agentic_harness_init?(): void;
}
"#
}

fn cargo_metadata(manifest: &Path) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--no-deps")
        .arg("--manifest-path")
        .arg(manifest)
        .output()?;
    if !output.status.success() {
        io::stderr().write_all(&output.stderr)?;
        return Err("cargo metadata failed".into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

fn parse_env_files(
    env_files: &[PathBuf],
) -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    let mut env = Vec::new();
    for file in env_files {
        let content = fs::read_to_string(file)?;
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            env.push((
                key.trim().to_string(),
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            ));
        }
    }
    Ok(env)
}

fn add_command(
    name: Option<String>,
    category: Option<String>,
    print: bool,
) -> Result<u8, Box<dyn std::error::Error>> {
    let agent_mode = should_print_agent_instructions(print);
    if let Some(category) = category {
        let seed = name.ok_or("--category requires a URL or path argument")?;
        let roots = native_category_roots();
        let root = roots
            .iter()
            .find(|root| root.category == category)
            .ok_or_else(|| {
                format!(
                    "Unknown category \"{category}\". Known categories: {}",
                    roots
                        .iter()
                        .map(|root| root.category)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let body = root.body.replace("{{URL}}", &seed);
        if agent_mode {
            print!("{body}");
            if !body.ends_with('\n') {
                println!();
            }
        } else {
            print_human_add_hint(&format!("agentic-harness add {seed} --category {category}"));
        }
        return Ok(0);
    }

    let connectors = native_connectors();
    match name.as_deref() {
        None => {
            println!("agentic-harness add <name>\n\nAvailable native connectors:");
            for connector in connectors {
                println!(
                    "  agentic-harness add {:<10} {:<8} {:<34} {}",
                    connector.name, connector.category, connector.website, connector.summary
                );
            }
            let roots = native_category_roots();
            if !roots.is_empty() {
                println!("\nDon't see what you need?");
                for root in roots {
                    println!(
                        "  agentic-harness add <url> --category {:<8} {}",
                        root.category, root.summary
                    );
                }
            }
            Ok(0)
        }
        Some(name) => {
            let connector = resolve_native_connector(&connectors, name).ok_or_else(|| {
                format!(
                    "Connector \"{name}\" not found. Available: {}",
                    connectors
                        .iter()
                        .map(|connector| connector.name)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
            if agent_mode {
                print!("{}", connector.body);
                if !connector.body.ends_with('\n') {
                    println!();
                }
            } else {
                print_human_add_hint(&format!("agentic-harness add {}", connector.name));
            }
            Ok(0)
        }
    }
}

fn resolve_native_connector<'a>(
    connectors: &'a [NativeConnector],
    name: &str,
) -> Option<&'a NativeConnector> {
    connectors
        .iter()
        .find(|connector| connector.name == name || connector.aliases.contains(&name))
        .or_else(|| {
            let lower = name.to_lowercase();
            connectors.iter().find(|connector| {
                connector.name.to_lowercase() == lower
                    || connector
                        .aliases
                        .iter()
                        .any(|alias| alias.to_lowercase() == lower)
            })
        })
}

fn should_print_agent_instructions(print: bool) -> bool {
    should_print_agent_instructions_for(
        print,
        io::stdout().is_terminal(),
        std::env::var_os("AGENTIC_HARNESS_FORCE_HUMAN_ADD_HINT").is_some(),
        std::env::vars(),
    )
}

fn should_print_agent_instructions_for<I, K, V>(
    print: bool,
    stdout_is_terminal: bool,
    force_human_hint: bool,
    envs: I,
) -> bool
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    if print {
        return true;
    }
    if force_human_hint {
        return false;
    }
    if !stdout_is_terminal {
        return true;
    }
    has_coding_agent_env(envs)
}

fn has_coding_agent_env<I, K, V>(envs: I) -> bool
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    const KNOWN_AGENT_ENV_KEYS: &[&str] = &[
        "CLAUDECODE",
        "CLAUDE_CODE",
        "CODEX_SANDBOX",
        "CODEX_ENV_PWD",
        "CURSOR_AGENT",
        "CURSOR_TRACE_ID",
        "OPENCODE",
        "OPENCODE_SESSION",
    ];

    envs.into_iter().any(|(key, value)| {
        let key = key.as_ref();
        let value = value.as_ref();
        !value.is_empty() && KNOWN_AGENT_ENV_KEYS.contains(&key)
    })
}

fn print_human_add_hint(command: &str) {
    eprintln!("{command}\n");
    eprintln!("To install this connector, pipe it to your coding agent:\n");
    eprintln!("  {command} --print | codex");
    eprintln!("  {command} --print | claude");
    eprintln!("  {command} --print | cursor-agent");
    eprintln!("  {command} --print | opencode");
    eprintln!("  {command} --print | pi");
}

fn native_model_connector_markdown() -> &'static str {
    r#"# Agentic Harness Model Connector

Use the built-in OpenAI-compatible client when the provider exposes a chat-completions-compatible API:

```rust
use agentic_harness::{AgentApp, OpenAiCompatibleModel, ProviderSettings};

let settings = ProviderSettings::new()
    .base_url("https://api.openai.com/v1")
    .api_key_env("OPENAI_API_KEY");

let app = AgentApp::new()
    .model("openai/gpt-5.5", OpenAiCompatibleModel::new("gpt-5.5", settings))
    .default_model("openai/gpt-5.5");
```

For non-native runtimes, provide the HTTP layer explicitly with `OpenAiCompatibleModel::with_transport(...)`. The transport should implement `agentic_harness::HttpModelTransport` and forward JSON through the host runtime's HTTP primitive, such as Worker `fetch`.

For non-compatible providers, implement `agentic_harness::ModelClient` directly. `ModelRequest` includes `tools: Vec<ToolSpec>` so providers that support tool/function calling can translate those specs into their native request format.

```rust
use agentic_harness::{AgenticHarnessError, ModelClient, ModelRequest, PromptResponse};

pub struct ProviderModel;

impl ModelClient for ProviderModel {
    fn complete(&self, request: ModelRequest) -> Result<PromptResponse, AgenticHarnessError> {
        let prompt = request.messages.last().map(|m| m.content.as_str()).unwrap_or("");
        let tool_count = request.tools.len();
        Ok(PromptResponse::text(format!("{prompt} ({tool_count} tools)")))
    }
}
```

Keep API keys in Rust-owned env/config, not in prompt text. Use `api_key_env(...)` for runtime env lookup or `api_key(...)` for explicit configured values.
"#
}

fn native_sandbox_connector_markdown() -> &'static str {
    "# Agentic Harness Sandbox Connector\n\nYou are an AI coding agent building a Rust-native sandbox connector for Agentic Harness.\n\nImplement a Rust type that satisfies the `agentic_harness::SessionEnv` contract:\n\n- `exec(command, ShellOptions)` for command execution with cwd/env/timeout.\n- `read_file`, `write_file`, `stat`, `readdir`, `exists`, `mkdir`, and `rm` for filesystem access.\n- `cwd` and `resolve_path` for workspace-relative paths.\n\nBind the connector inside a handler with `ctx.session_with_id_and_env(\"remote\", env)` so `session.shell`, `session.read`, `session.write`, `session.grep`, and related helpers execute inside the connector environment. Use `AgentContext::shell_with_options` and the file helpers as the local reference implementation. Keep provider credentials outside prompt text and inject secrets only through explicit `ShellOptions::env(...)` calls or the provider's control plane.\n"
}

struct NativeConnector {
    name: &'static str,
    category: &'static str,
    website: &'static str,
    aliases: &'static [&'static str],
    summary: &'static str,
    body: &'static str,
}

struct NativeCategoryRoot {
    category: &'static str,
    summary: &'static str,
    body: &'static str,
}

fn native_category_roots() -> Vec<NativeCategoryRoot> {
    vec![NativeCategoryRoot {
        category: "sandbox",
        summary: "Build a sandbox connector from scratch",
        body: native_sandbox_category_markdown(),
    }]
}

fn native_connectors() -> Vec<NativeConnector> {
    vec![
        NativeConnector {
            name: "model",
            category: "model",
            website: "https://openai.com",
            aliases: &["provider"],
            summary: "Provider-backed ModelClient adapter",
            body: native_model_connector_markdown(),
        },
        NativeConnector {
            name: "sandbox",
            category: "sandbox",
            website: "native",
            aliases: &[],
            summary: "Native sandbox/session environment adapter",
            body: native_sandbox_connector_markdown(),
        },
        NativeConnector {
            name: "mcp",
            category: "mcp",
            website: "https://modelcontextprotocol.io",
            aliases: &["model-context-protocol"],
            summary: "Streamable-HTTP MCP server tool adapter",
            body: native_mcp_connector_markdown(),
        },
        NativeConnector {
            name: "daytona",
            category: "sandbox",
            website: "https://daytona.io",
            aliases: &["sandbox-daytona"],
            summary: "Rust-native Daytona sandbox connector instructions",
            body: native_daytona_connector_markdown(),
        },
        NativeConnector {
            name: "e2b",
            category: "sandbox",
            website: "https://e2b.dev",
            aliases: &["sandbox-e2b", "e2b-sandbox"],
            summary: "Rust-native E2B sandbox connector instructions",
            body: native_e2b_connector_markdown(),
        },
        NativeConnector {
            name: "vercel",
            category: "sandbox",
            website: "https://vercel.com",
            aliases: &["@vercel/sandbox", "sandbox-vercel"],
            summary: "Hosted coding sandbox target",
            body: native_vercel_connector_markdown(),
        },
    ]
}

fn native_sandbox_category_markdown() -> &'static str {
    "# Generic Agentic Harness Sandbox Connector\n\nYou are an AI coding agent being asked to build a Rust-native Agentic Harness sandbox connector for a provider that does not have a built-in recipe yet.\n\n## Starting point\n\nThe user passed this provider documentation URL or path:\n\n`{{URL}}`\n\nTreat it as a starting hint, then inspect the provider's current Rust SDK, HTTP API, CLI, or docs well enough to implement the connector accurately.\n\n## Contract\n\nBuild a Rust type that implements `agentic_harness::SessionEnv`:\n\n- `exec(command, ShellOptions)` maps command execution, cwd, env, and timeout.\n- `read_file`, `write_file`, `stat`, `readdir`, `exists`, `mkdir`, and `rm` map filesystem operations.\n- `cwd` and `resolve_path` preserve workspace-relative behavior.\n\nUse `AgentContext::shell_with_options` and the local `SessionEnv for AgentContext` implementation as the behavior reference. If the provider SDK is async-only, keep the async boundary narrow and explicit in the user's project.\n\n## File placement\n\nPrefer `src/connectors/<provider>.rs` in the user's Rust agent project. Wire it into the agent handler only after the connector compiles.\n\n## Wiring\n\nCreate or acquire the provider sandbox in trusted Rust code, then bind it to a session:\n\n```rust\nlet env = ProviderEnv::new(/* provider client/sandbox */);\nlet session = ctx.session_with_id_and_env(\"remote\", env);\nlet output = session.shell(\"pwd\")?;\n```\n\nAfter binding, `session.shell`, `session.read`, `session.write`, `session.grep`, `session.glob`, and related helpers use the provider environment instead of the host workspace.\n\n## Credentials\n\nNever invent API keys, tokens, or secrets. Use the provider's auth layer, existing env conventions, or explicit `ShellOptions::env(...)` values. If no project convention exists, ask the user before adding secret names or config files.\n\n## Verify\n\nRun the user's Rust checks, at minimum `cargo fmt --check`, `cargo test`, and `cargo clippy -- -D warnings` when available. Then show a small usage snippet that passes the connector-backed environment into the Agentic Harness app or handler.\n"
}

fn native_mcp_connector_markdown() -> &'static str {
    "# Agentic Harness MCP Connector\n\nUse Agentic Harness's Rust-native MCP adapter to expose a streamable-HTTP MCP server as ordinary `ToolDef`s:\n\n```rust\nuse agentic_harness::{connect_mcp_server, AgentApp, AgenticHarnessError, McpServerOptions};\n\nfn app() -> Result<AgentApp, AgenticHarnessError> {\n    let mcp = connect_mcp_server(\n        \"docs\",\n        McpServerOptions::new(\"https://mcp.example.com/mcp\")\n            .header(\"Authorization\", format!(\"Bearer {}\", std::env::var(\"MCP_TOKEN\").unwrap_or_default())),\n    )?;\n\n    let app = mcp\n        .tools\n        .into_iter()\n        .fold(AgentApp::new(), |app, tool| app.tool(tool));\n\n    Ok(app)\n}\n```\n\nFor legacy MCP SSE servers, select the old transport explicitly:\n\n```rust\nuse agentic_harness::{McpServerOptions, McpTransport};\n\nlet options = McpServerOptions::new(\"https://mcp.example.com/sse\")\n    .transport(McpTransport::Sse);\n```\n\nTool names are sanitized as `mcp__<server>__<tool>`. MCP result content is formatted into text before it is returned to the model. Keep MCP credentials in Rust env/config, not in prompts.\n"
}

fn native_daytona_connector_markdown() -> &'static str {
    "# Rust-native Daytona sandbox connector\n\nYou are an AI coding agent installing a Daytona sandbox connector for native Agentic Harness.\n\nBuild a small Rust module, for example `src/connectors/daytona.rs`, that wraps the user's initialized Daytona sandbox/client behind Agentic Harness's `SessionEnv` shape:\n\n```rust\nuse agentic_harness::{FileStat, AgenticHarnessError, SessionEnv, ShellOptions, ShellOutput};\nuse std::path::{Path, PathBuf};\n\npub struct DaytonaEnv {\n    cwd: PathBuf,\n    // Store the user's Daytona sandbox/client handle here.\n}\n\nimpl SessionEnv for DaytonaEnv {\n    fn exec(&self, command: &str, options: ShellOptions) -> Result<ShellOutput, AgenticHarnessError> {\n        // Forward command, options.cwd, options.env, and options.timeout to Daytona.\n        // Return stdout, stderr, and exit_code without exposing host secrets.\n        todo!(\"wire Daytona command execution\")\n    }\n\n    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> { todo!(\"download/read file\") }\n    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> { todo!(\"upload/write file\") }\n    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> { todo!(\"map Daytona file details\") }\n    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> { todo!(\"list files\") }\n    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> { todo!(\"check file details\") }\n    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> { todo!(\"create folder\") }\n    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> { todo!(\"delete file/folder\") }\n    fn cwd(&self) -> &Path { &self.cwd }\n    fn resolve_path(&self, path: &str) -> PathBuf { if Path::new(path).is_absolute() { path.into() } else { self.cwd.join(path) } }\n}\n```\n\nWire it by creating `DaytonaEnv` in trusted Rust code and using `ctx.session_with_id_and_env(\"daytona\", daytona_env)`. After that, the normal session helpers run in Daytona. If the provider SDK is async-only, keep the async client behind a narrow sync boundary chosen by the project, or expose an async connector module and call it from handlers before passing results to Agentic Harness. Use `AgentContext::shell_with_options` as the local behavior reference for cwd/env/timeout semantics.\n"
}

fn native_e2b_connector_markdown() -> &'static str {
    r#"# Rust-native E2B sandbox connector

You are an AI coding agent installing an E2B sandbox connector for native Agentic Harness. Use this when the coding agent needs an isolated cloud Linux environment with command execution and filesystem access.

Build a small Rust module, for example `src/connectors/e2b.rs`, that adapts the user's E2B sandbox/client handle into `agentic_harness::SessionEnv`.

Required behavior:

- `exec(command, ShellOptions)` must map `ShellOptions::cwd`, `ShellOptions::env`, and `ShellOptions::timeout` onto E2B command execution.
- `read_file`, `write_file`, `stat`, `readdir`, `exists`, `mkdir`, and `rm` must operate inside the E2B filesystem.
- `cwd` and `resolve_path` must preserve workspace-relative behavior.
- Return accurate `ShellOutput { stdout, stderr, exit_code }` and `FileStat` metadata.
- Keep API keys and sandbox credentials in trusted Rust env/config, never in prompt text.

Skeleton:

```rust
use agentic_harness::{AgenticHarnessError, FileStat, SessionEnv, ShellOptions, ShellOutput};
use std::path::{Path, PathBuf};

pub struct E2bEnv {
    cwd: PathBuf,
    // Store the user's E2B sandbox/client handle here.
}

impl SessionEnv for E2bEnv {
    fn exec(&self, command: &str, options: ShellOptions) -> Result<ShellOutput, AgenticHarnessError> {
        // Forward command, ShellOptions::cwd, ShellOptions::env, and ShellOptions::timeout to E2B.
        todo!("wire E2B command execution")
    }

    fn read_file(&self, path: &str) -> Result<String, AgenticHarnessError> { todo!("read file from E2B") }
    fn write_file(&self, path: &str, content: &[u8]) -> Result<(), AgenticHarnessError> { todo!("write file to E2B") }
    fn stat(&self, path: &str) -> Result<FileStat, AgenticHarnessError> { todo!("map E2B file details") }
    fn readdir(&self, path: &str) -> Result<Vec<String>, AgenticHarnessError> { todo!("list E2B files") }
    fn exists(&self, path: &str) -> Result<bool, AgenticHarnessError> { todo!("check E2B path") }
    fn mkdir(&self, path: &str) -> Result<(), AgenticHarnessError> { todo!("create E2B directory") }
    fn rm(&self, path: &str, recursive: bool) -> Result<(), AgenticHarnessError> { todo!("delete E2B path") }
    fn cwd(&self) -> &Path { &self.cwd }
    fn resolve_path(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() { path.to_path_buf() } else { self.cwd.join(path) }
    }
}
```

Wire it from trusted handler code:

```rust
let e2b_env = E2bEnv::new(/* sandbox client/handle */);
let session = ctx.session_with_id_and_env("e2b", e2b_env);
let test = session.shell("cargo test")?;
```

After binding, `session.shell`, `session.read`, `session.write`, `session.grep`, `session.glob`, and related helpers execute inside E2B. Verify with `cargo fmt --check`, `cargo test`, and `cargo clippy -- -D warnings` when available.
"#
}

fn native_vercel_connector_markdown() -> &'static str {
    r#"# Rust-native Vercel Sandbox connector

You are an AI coding agent installing a Vercel Sandbox connector for native Agentic Harness. Use this as the first-class hosted coding target when a task needs an isolated Linux filesystem and shell.

Create or reuse a Vercel Sandbox in trusted Rust code, then wrap it in a `SessionEnv` implementation.

Build a Rust module that adapts the user's Vercel Sandbox control surface into `agentic_harness::SessionEnv`. Match the core coding-agent operations: command execution, read/write, stat, readdir, exists, mkdir, rm, cwd, and path resolution.

Important requirements:

- Map `ShellOptions::cwd`, `ShellOptions::env`, and `ShellOptions::timeout` onto the Vercel sandbox command API.
- Never pass host API keys into prompt text. Use the provider's auth/config layer or explicit command env only.
- Return `ShellOutput { stdout, stderr, exit_code }` and `FileStat` with accurate file/directory/symlink/size metadata.
- Keep the connector as ordinary Rust source in the user's project, not a generated adapter.

Use `ctx.shell_with_options(...)` and the `SessionEnv for AgentContext` implementation as the local reference while wiring the remote provider:

```rust
let vercel_env = VercelSandboxEnv::new(/* sandbox client/handle */);
let session = ctx.session_with_id_and_env("project", vercel_env);
let status = session.shell("cargo test")?;
```

After binding, normal session helpers execute inside Vercel Sandbox.
"#
}

#[derive(Debug)]
struct TemplatePack {
    root: PathBuf,
    name: String,
    agent_name: String,
    version: String,
    description: String,
    sample_payload: String,
}

fn load_template_pack(path: &Path) -> Result<TemplatePack, Box<dyn std::error::Error>> {
    let manifest = path.join("agentic-template.toml");
    let text = fs::read_to_string(&manifest)
        .map_err(|err| format!("Could not read {}: {err}", manifest.display()))?;
    let name = parse_manifest_string(&text, "name")?;
    let agent_name = parse_manifest_string(&text, "agent")?;
    let version = parse_manifest_string(&text, "version").unwrap_or_else(|_| "0.1.0".to_string());
    let description = parse_manifest_string(&text, "description")?;
    let sample_payload =
        parse_manifest_string(&text, "sample_payload").unwrap_or_else(|_| "{}".to_string());
    Ok(TemplatePack {
        root: path.to_path_buf(),
        name,
        agent_name,
        version,
        description,
        sample_payload,
    })
}

fn parse_manifest_string(text: &str, key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let prefix = format!("{key} = ");
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with(&prefix))
        .ok_or_else(|| format!("agentic-template.toml is missing `{key}`"))?;
    let value = line
        .strip_prefix(&prefix)
        .ok_or_else(|| format!("invalid manifest key `{key}`"))?
        .trim();
    if !(value.starts_with('"') && value.ends_with('"')) {
        return Err(format!("manifest `{key}` must be a quoted string").into());
    }
    Ok(unescape_manifest_string(&value[1..value.len() - 1]))
}

fn unescape_manifest_string(value: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            match ch {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                other => out.push(other),
            }
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    out
}

fn validate_template_pack(pack: &TemplatePack) -> Result<(), Box<dyn std::error::Error>> {
    if pack.name.trim().is_empty() {
        return Err("template name cannot be empty".into());
    }
    validate_package_name(&pack.name)?;
    if pack.agent_name.trim().is_empty() {
        return Err("template agent cannot be empty".into());
    }
    validate_package_name(&pack.agent_name)?;
    validate_template_version(&pack.version)?;
    serde_json::from_str::<serde_json::Value>(&pack.sample_payload)
        .map_err(|err| format!("sample_payload must be valid JSON: {err}"))?;
    let required = [
        "agentic-template.toml",
        "files/Cargo.toml.hbs",
        "files/src/main.rs.hbs",
        "files/AGENTS.md.hbs",
        "examples/payload.json",
    ];
    for relative in required {
        let path = pack.root.join(relative);
        if !path.exists() {
            return Err(format!("template is missing {relative}").into());
        }
    }
    serde_json::from_str::<serde_json::Value>(&fs::read_to_string(
        pack.root.join("examples/payload.json"),
    )?)
    .map_err(|err| format!("examples/payload.json must be valid JSON: {err}"))?;
    Ok(())
}

fn validate_template_version(version: &str) -> Result<(), Box<dyn std::error::Error>> {
    let value = version.trim();
    if value.is_empty() {
        return Err("template version cannot be empty".into());
    }
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '+'))
    {
        Ok(())
    } else {
        Err(format!(
            "invalid template version {version:?}; use ASCII letters, digits, '.', '-' or '+'"
        )
        .into())
    }
}

fn init_template_pack(
    path: &Path,
    name: Option<String>,
    agent: Option<String>,
    description: &str,
    version: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() && path.read_dir()?.next().is_some() {
        return Err(format!("{} already exists and is not empty", path.display()).into());
    }
    let template_name = name.unwrap_or_else(|| {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agent-template")
            .to_string()
    });
    validate_package_name(&template_name)?;
    let agent_name = agent.unwrap_or_else(|| template_name.clone());
    validate_package_name(&agent_name)?;
    validate_template_version(version)?;
    let description = if description.trim().is_empty() {
        "Reusable Agentic Harness template"
    } else {
        description.trim()
    };
    let sample_payload = serde_json::json!({
        "prompt": format!("Run the {agent_name} workflow")
    })
    .to_string();

    fs::create_dir_all(path.join("files/src"))?;
    fs::create_dir_all(path.join("files/.agentic-harness/roles"))?;
    fs::create_dir_all(path.join("files/.agents/skills").join(&agent_name))?;
    fs::create_dir_all(path.join("examples"))?;
    fs::write(
        path.join("agentic-template.toml"),
        format!(
            "name = \"{}\"\nagent = \"{}\"\nversion = \"{}\"\ndescription = \"{}\"\nsample_payload = \"{}\"\n",
            escape_template_manifest_string(&template_name),
            escape_template_manifest_string(&agent_name),
            escape_template_manifest_string(version.trim()),
            escape_template_manifest_string(description),
            escape_template_manifest_string(&sample_payload)
        ),
    )?;
    fs::write(
        path.join("files/Cargo.toml.hbs"),
        template_init_cargo_toml_hbs(),
    )?;
    fs::write(
        path.join("files/src/main.rs.hbs"),
        template_init_main_rs_hbs(&agent_name),
    )?;
    fs::write(
        path.join("files/AGENTS.md.hbs"),
        template_init_agents_md_hbs(&template_name, description),
    )?;
    fs::write(
        path.join("files/.agentic-harness/roles")
            .join(format!("{agent_name}.md")),
        format!(
            "---\ndescription: {}\n---\nYou are the `{}` role for this Agentic Harness template.\n",
            description, agent_name
        ),
    )?;
    fs::write(
        path.join("files/.agents/skills")
            .join(&agent_name)
            .join("SKILL.md"),
        format!(
            "---\nname: {}\ndescription: {}\n---\n\nInspect the workspace, make a focused plan, return `generatedPatch` when the agent should edit files, run relevant checks, and summarize the result.\n",
            agent_name, description
        ),
    )?;
    fs::write(
        path.join("examples/payload.json"),
        serde_json::to_string_pretty(&serde_json::from_str::<serde_json::Value>(&sample_payload)?)?,
    )?;
    println!(
        "[agentic-harness] Created template pack {} at {}",
        template_name,
        path.display()
    );
    println!("next: agentic-harness template validate {}", path.display());
    Ok(())
}

fn template_init_cargo_toml_hbs() -> &'static str {
    r#"[package]
name = "{{package_name}}"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = { path = "{{crate_path}}" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
"#
}

fn template_init_main_rs_hbs(agent_name: &str) -> String {
    format!(
        r#"use agentic_harness::{{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError}};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct TemplatePayload {{
    prompt: Option<String>,
}}

fn app() -> Result<AgentApp, AgenticHarnessError> {{
    Ok(AgentApp::new()
        .with_workspace(".")
        .load_workspace_context()?
        .agent(AgentDefinition::webhook("{{{{agent_name}}}}", |ctx: AgentContext| {{
            let payload: TemplatePayload = ctx.payload()?;
            Ok(json!({{
                "id": ctx.id(),
                "agent": "{agent_name}",
                "prompt": payload.prompt.unwrap_or_else(|| "Run this workflow".to_string()),
                "summary": "Template scaffold executed. Replace this handler with project-specific logic."
            }}))
        }})))
}}

fn main() {{
    let code = match app().and_then(run_cli) {{
        Ok(code) => code,
        Err(err) => {{
            eprintln!("[agentic-harness] {{err}}");
            1
        }}
    }};
    std::process::exit(code);
}}
"#
    )
}

fn template_init_agents_md_hbs(template_name: &str, description: &str) -> String {
    format!(
        "# {{{{package_name}}}}\n\nThis project was scaffolded from the `{template_name}` Agentic Harness template.\n\nPurpose: {description}\n\nUse the included role and skill files as the first place to customize behavior.\n"
    )
}

fn escape_template_manifest_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

fn format_template_preview_json(
    template: &str,
    workspace: &Path,
) -> Result<String, Box<dyn std::error::Error>> {
    if let Ok(template) = ScaffoldTemplate::parse(template) {
        let files = template
            .preview_files()
            .iter()
            .map(|file| (*file).to_string())
            .collect::<Vec<_>>();
        let value = serde_json::json!({
            "name": template.name(),
            "source": "built-in",
            "path": serde_json::Value::Null,
            "agent": template.agent_name(),
            "version": env!("CARGO_PKG_VERSION"),
            "description": template.description(),
            "samplePayload": scaffold_sample_payload(template),
            "valid": true,
            "fileCount": files.len(),
            "files": files,
            "nextCommand": format!("agentic-harness new ./my-agent --template {}", template.name()),
        });
        return Ok(format!("{}\n", serde_json::to_string_pretty(&value)?));
    }

    let (pack_path, source) = resolve_template_pack_from(workspace, template)?;
    let pack = load_template_pack(&pack_path)?;
    validate_template_pack(&pack)?;
    let files = collect_template_pack_files(&pack.root)?;
    let path = pack
        .root
        .canonicalize()
        .unwrap_or_else(|_| pack.root.to_path_buf());
    let value = serde_json::json!({
        "name": &pack.name,
        "source": source,
        "path": path.display().to_string(),
        "agent": &pack.agent_name,
        "version": &pack.version,
        "description": &pack.description,
        "samplePayload": &pack.sample_payload,
        "valid": true,
        "fileCount": files.len(),
        "files": files,
        "nextCommand": format!("agentic-harness new ./my-agent --template {}", pack.name),
    });
    Ok(format!("{}\n", serde_json::to_string_pretty(&value)?))
}

fn print_template_preview(
    template: &str,
    workspace: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(template) = ScaffoldTemplate::parse(template) {
        println!("Template: {}", template.name());
        println!("Source: built-in");
        println!("Agent: {}", template.agent_name());
        println!("Description: {}", template.description());
        println!("Sample payload: {}", scaffold_sample_payload(template));
        println!("Files:");
        for file in template.preview_files() {
            println!("  - {file}");
        }
        println!(
            "\nNext: agentic-harness new ./my-agent --template {}",
            template.name()
        );
        return Ok(());
    }

    let (pack_path, source) = resolve_template_pack_from(workspace, template)?;
    let pack = load_template_pack(&pack_path)?;
    validate_template_pack(&pack)?;
    println!("Template: {}", pack.name);
    println!("Source: {source}");
    println!("Path: {}", pack.root.display());
    println!("Agent: {}", pack.agent_name);
    println!("Version: {}", pack.version);
    println!("Description: {}", pack.description);
    println!("Sample payload: {}", pack.sample_payload);
    println!("Files:");
    for file in collect_template_pack_files(&pack.root)? {
        println!("  - {file}");
    }
    println!(
        "\nNext: agentic-harness new ./my-agent --template {}",
        pack.name
    );
    Ok(())
}

fn collect_template_pack_files(root: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let files_root = root.join("files");
    let mut files = Vec::new();
    collect_template_pack_files_inner(&files_root, &files_root, &mut files)?;
    files.sort();
    files.truncate(80);
    Ok(files)
}

fn collect_template_pack_files_inner(
    root: &Path,
    path: &Path,
    files: &mut Vec<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_template_pack_files_inner(root, &path, files)?;
        } else {
            files.push(format!("files/{}", path.strip_prefix(root)?.display()));
        }
    }
    Ok(())
}

fn validate_package_name(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    if name
        .chars()
        .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_')
    {
        Ok(())
    } else {
        Err(format!(
            "invalid package/template name {name:?}; use lowercase ASCII, digits, '-' or '_'"
        )
        .into())
    }
}

fn scaffold_any_template(
    path: &Path,
    name: Option<String>,
    template: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match ScaffoldTemplate::parse(template) {
        Ok(template) => scaffold(path, name, template),
        Err(_) => {
            let pack_path = resolve_template_pack(template)?;
            let pack = load_template_pack(&pack_path)?;
            validate_template_pack(&pack)?;
            scaffold_template_pack(path, name, &pack)
        }
    }
}

fn resolve_template_pack(template: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    resolve_template_pack_from(&std::env::current_dir()?, template).map(|(path, _)| path)
}

fn resolve_template_pack_from(
    workspace: &Path,
    template: &str,
) -> Result<(PathBuf, &'static str), Box<dyn std::error::Error>> {
    let direct = PathBuf::from(template);
    if direct.join("agentic-template.toml").exists() {
        return Ok((direct, "path"));
    }
    for scope in [
        TemplateScope::Workspace,
        TemplateScope::User,
        TemplateScope::Team,
    ] {
        let installed = scope.dir(workspace).join(template);
        if installed.join("agentic-template.toml").exists() {
            return Ok((installed, scope.source()));
        }
    }
    Err(format!(
        "Unknown template \"{template}\". Use a built-in template, workspace/user/team template, or template pack path."
    )
    .into())
}

fn scaffold_template_pack(
    path: &Path,
    name: Option<String>,
    pack: &TemplatePack,
) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() && path.read_dir()?.next().is_some() {
        return Err(format!("{} already exists and is not empty", path.display()).into());
    }
    let package_name = name.unwrap_or_else(|| {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agentic-harness-agent")
            .to_string()
    });
    validate_package_name(&package_name)?;
    let crate_path = agentic_harness_crate_path()?;
    let crate_path_text = crate_path.display().to_string();
    render_template_dir(
        &pack.root.join("files"),
        path,
        &[
            ("package_name", package_name.as_str()),
            ("template_name", pack.name.as_str()),
            ("agent_name", pack.agent_name.as_str()),
            ("crate_path", crate_path_text.as_str()),
        ],
    )?;
    eprintln!(
        "[agentic-harness] Created Agentic Harness {} project at {}",
        pack.name,
        path.display()
    );
    eprintln!(
        "[agentic-harness] Try: agentic-harness run {} --workspace {} --id demo --payload '{}'",
        pack.agent_name,
        path.display(),
        pack.sample_payload
    );
    Ok(())
}

fn render_template_dir(
    source: &Path,
    destination: &Path,
    values: &[(&str, &str)],
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let relative = source_path.strip_prefix(source)?;
        let mut destination_path = destination.join(relative);
        if source_path.is_dir() {
            fs::create_dir_all(&destination_path)?;
            render_template_dir(&source_path, &destination_path, values)?;
        } else {
            if destination_path
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("hbs")
            {
                destination_path.set_extension("");
            }
            if let Some(parent) = destination_path.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut content = fs::read_to_string(&source_path)?;
            for (key, value) in values {
                content = content.replace(&format!("{{{{{key}}}}}"), value);
            }
            fs::write(destination_path, content)?;
        }
    }
    Ok(())
}

fn copy_dir_all(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir_all(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScaffoldTemplate {
    Hello,
    Triage,
    Data,
    Coding,
    CodeReview,
    TestFixer,
    RepoAnalyst,
    Support,
}

const BUILT_IN_TEMPLATES: &[ScaffoldTemplate] = &[
    ScaffoldTemplate::Hello,
    ScaffoldTemplate::Triage,
    ScaffoldTemplate::Data,
    ScaffoldTemplate::Coding,
    ScaffoldTemplate::CodeReview,
    ScaffoldTemplate::TestFixer,
    ScaffoldTemplate::RepoAnalyst,
    ScaffoldTemplate::Support,
];

impl ScaffoldTemplate {
    fn parse(value: &str) -> Result<Self, Box<dyn std::error::Error>> {
        match value {
            "hello" => Ok(Self::Hello),
            "triage" => Ok(Self::Triage),
            "data" => Ok(Self::Data),
            "coding" => Ok(Self::Coding),
            "code-review" => Ok(Self::CodeReview),
            "test-fixer" => Ok(Self::TestFixer),
            "repo-analyst" => Ok(Self::RepoAnalyst),
            "support" => Ok(Self::Support),
            other => Err(format!(
                "Unknown template \"{other}\". Available templates: hello, triage, data, coding, code-review, test-fixer, repo-analyst, support"
            )
            .into()),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::Triage => "triage",
            Self::Data => "data",
            Self::Coding => "coding",
            Self::CodeReview => "code-review",
            Self::TestFixer => "test-fixer",
            Self::RepoAnalyst => "repo-analyst",
            Self::Support => "support",
        }
    }

    fn agent_name(self) -> &'static str {
        match self {
            Self::Hello => "hello",
            Self::Triage => "triage",
            Self::Data => "data",
            Self::Coding => "code",
            Self::CodeReview => "code-review",
            Self::TestFixer => "test-fixer",
            Self::RepoAnalyst => "repo-analyst",
            Self::Support => "support",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Hello => "Minimal webhook agent for a typed JSON request.",
            Self::Triage => "CLI-only issue triage agent for CI and local automation.",
            Self::Data => "Workspace-aware data agent with local file listing.",
            Self::Coding => {
                "Repository coding agent that inspects status, plans work, and runs checks."
            }
            Self::CodeReview => {
                "Repository review agent that inspects status, diffs, checks, and risks."
            }
            Self::TestFixer => {
                "Focused test repair agent that turns failing checks into a repair plan."
            }
            Self::RepoAnalyst => {
                "Repository analyst agent that inventories files, entrypoints, and project shape."
            }
            Self::Support => "Customer support agent starter with a support skill and role.",
        }
    }

    fn preview_files(self) -> &'static [&'static str] {
        match self {
            Self::Hello
            | Self::Triage
            | Self::Data
            | Self::Coding
            | Self::CodeReview
            | Self::TestFixer
            | Self::RepoAnalyst
            | Self::Support => &[
                "Cargo.toml",
                "src/main.rs",
                "AGENTS.md",
                ".agentic-harness/roles/<role>.md",
                ".agents/skills/<template>/SKILL.md",
            ],
        }
    }

    fn main_rs(self) -> &'static str {
        match self {
            Self::Hello => scaffold_hello_main_rs(),
            Self::Triage => scaffold_triage_main_rs(),
            Self::Data => scaffold_data_main_rs(),
            Self::Coding => scaffold_coding_main_rs(),
            Self::CodeReview => scaffold_code_review_main_rs(),
            Self::TestFixer => scaffold_test_fixer_main_rs(),
            Self::RepoAnalyst => scaffold_repo_analyst_main_rs(),
            Self::Support => scaffold_support_main_rs(),
        }
    }
}

fn scaffold(
    path: &Path,
    name: Option<String>,
    template: ScaffoldTemplate,
) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() && path.read_dir()?.next().is_some() {
        return Err(format!("{} already exists and is not empty", path.display()).into());
    }

    let package_name = name.unwrap_or_else(|| {
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("agentic-harness-agent")
            .to_string()
    });
    let agentic_harness_path = agentic_harness_crate_path()?;

    fs::create_dir_all(path.join("src"))?;
    fs::create_dir_all(path.join(".agentic-harness/roles"))?;
    fs::create_dir_all(path.join(".agents/skills").join(template.name()))?;
    fs::write(
        path.join("Cargo.toml"),
        format!(
            r#"[package]
name = "{package_name}"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#,
            agentic_harness_path.display()
        ),
    )?;
    fs::write(path.join("src/main.rs"), template.main_rs())?;
    fs::write(
        path.join("AGENTS.md"),
        format!(
            "You are a native Agentic Harness {} agent. Keep responses concise and structured.\n",
            template.name()
        ),
    )?;
    fs::write(
        path.join(".agentic-harness/roles/greeter.md"),
        "---\ndescription: Warm concise greeter\n---\nUse a direct, friendly greeting.\n",
    )?;
    fs::write(
        path.join(".agentic-harness/roles/reviewer.md"),
        "---\ndescription: Careful implementation reviewer\n---\nCheck risks, tests, and user-visible behavior before answering.\n",
    )?;
    fs::write(
        path.join(".agents/skills")
            .join(template.name())
            .join("SKILL.md"),
        format!(
            "---\nname: {}\ndescription: Starter workflow for the {} agent template\n---\nUse the workspace context, inspect files before claims, and return a concise structured result.\n",
            template.name(),
            template.name()
        ),
    )?;

    eprintln!(
        "[agentic-harness] Created native Agentic Harness {} project at {}",
        template.name(),
        path.display()
    );
    eprintln!(
        "[agentic-harness] Try: agentic-harness run {} --workspace {} --id demo --payload '{}'",
        template.agent_name(),
        path.display(),
        scaffold_sample_payload(template)
    );
    Ok(())
}

fn agentic_harness_crate_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let cli_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    Ok(cli_dir
        .parent()
        .ok_or("could not resolve crates directory")?
        .join("agentic-harness")
        .canonicalize()?)
}

fn scaffold_sample_payload(template: ScaffoldTemplate) -> &'static str {
    match template {
        ScaffoldTemplate::Hello => "{\"name\":\"Ada\"}",
        ScaffoldTemplate::Triage => "{\"issue\":\"Button click fails on checkout\"}",
        ScaffoldTemplate::Data => "{\"message\":\"Summarize this workspace\"}",
        ScaffoldTemplate::Coding => {
            "{\"repo\":\".\",\"prompt\":\"Run the tests and summarize failures\"}"
        }
        ScaffoldTemplate::CodeReview => {
            "{\"prompt\":\"Review the current changes\",\"checks\":[\"cargo test\"]}"
        }
        ScaffoldTemplate::TestFixer => {
            "{\"failingCheck\":\"cargo test\",\"failureOutput\":\"paste failing output here\"}"
        }
        ScaffoldTemplate::RepoAnalyst => {
            "{\"question\":\"Map this repository and identify the main entrypoints\"}"
        }
        ScaffoldTemplate::Support => "{\"message\":\"I cannot sign in\"}",
    }
}

fn scaffold_hello_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct HelloPayload {
	name: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("hello", |ctx: AgentContext| {
			let payload: HelloPayload = ctx.payload()?;
			let name = payload.name.unwrap_or_else(|| "World".to_string());
			let role = ctx
				.role("greeter")
				.map(|role| role.description.clone())
				.unwrap_or_else(|| "default".to_string());
			Ok(json!({
				"id": ctx.id(),
				"message": format!("Hello, {name}!"),
				"role": role
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_triage_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct TriagePayload {
	issue: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::cli_only("triage", |ctx: AgentContext| {
			let payload: TriagePayload = ctx.payload()?;
			let issue = payload.issue.unwrap_or_else(|| "unspecified issue".to_string());
			Ok(json!({
				"id": ctx.id(),
				"issue": issue,
				"severity": "medium",
				"reproducible": false,
				"summary": "Triage template executed. Replace this deterministic response with model-backed analysis when ready."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_data_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct DataPayload {
	message: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("data", |ctx: AgentContext| {
			let payload: DataPayload = ctx.payload()?;
			let message = payload.message.unwrap_or_else(|| "Summarize this workspace".to_string());
			let listing = ctx.readdir(".").unwrap_or_default();
			Ok(json!({
				"id": ctx.id(),
				"question": message,
				"filesSeen": listing,
				"summary": "Data template executed with workspace file access."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_coding_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError, ShellOptions};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct CodingPayload {
	repo: Option<String>,
	prompt: Option<String>,
	#[serde(rename = "gitStatusBefore")]
	git_status_before: Option<String>,
	#[serde(rename = "gitDiffStat")]
	git_diff_stat: Option<String>,
	#[serde(rename = "gitChangedFiles")]
	git_changed_files: Option<Vec<String>>,
	#[serde(rename = "rootFiles")]
	root_files: Option<Vec<String>>,
	#[serde(rename = "projectFiles")]
	project_files: Option<Vec<String>>,
	#[serde(rename = "workspaceInstructions")]
	workspace_instructions: Option<Vec<WorkspaceInstructionPayload>>,
	checks: Option<Vec<String>>,
	#[serde(rename = "plannedSteps")]
	planned_steps: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Serialize)]
struct WorkspaceInstructionPayload {
	path: String,
	content: String,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("code", |ctx: AgentContext| {
			let payload: CodingPayload = ctx.payload()?;
			let repo = payload.repo.unwrap_or_else(|| ".".to_string());
			let prompt = payload.prompt.unwrap_or_else(|| "Inspect the project".to_string());
			let checks = payload.checks.unwrap_or_default();
			let planned_steps = payload.planned_steps.unwrap_or_else(|| {
				vec![
					"Read workspace instructions and current repository status.".to_string(),
					"Apply the smallest focused changes needed for the prompt.".to_string(),
					"Run the configured checks and summarize remaining risks.".to_string(),
				]
			});
			let status = ctx.shell_with_options(
				"git status --short || true",
				ShellOptions::new()
					.cwd(&repo)
					.timeout(Duration::from_secs(5)),
			)?;
			Ok(json!({
				"id": ctx.id(),
				"repo": repo,
				"prompt": prompt,
				"status": status.stdout,
				"inspect": {
					"gitStatusBefore": payload.git_status_before.unwrap_or_default(),
					"gitDiffStat": payload.git_diff_stat.unwrap_or_default(),
					"gitChangedFiles": payload.git_changed_files.unwrap_or_default(),
					"rootFiles": payload.root_files.unwrap_or_default(),
					"projectFiles": payload.project_files.unwrap_or_default(),
					"workspaceInstructions": payload.workspace_instructions.unwrap_or_default()
				},
				"plan": planned_steps,
				"plannedSteps": planned_steps,
				"checks": checks,
				"summary": "Coding loop inspected the repository and prepared a local check plan."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_code_review_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError, ShellOptions};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct ReviewPayload {
	prompt: Option<String>,
	checks: Option<Vec<String>>,
	#[serde(rename = "gitStatusBefore")]
	git_status_before: Option<String>,
	#[serde(rename = "gitDiffStat")]
	git_diff_stat: Option<String>,
	#[serde(rename = "gitChangedFiles")]
	git_changed_files: Option<Vec<String>>,
	#[serde(rename = "rootFiles")]
	root_files: Option<Vec<String>>,
	#[serde(rename = "projectFiles")]
	project_files: Option<Vec<String>>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("code-review", |ctx: AgentContext| {
			let payload: ReviewPayload = ctx.payload()?;
			let prompt = payload.prompt.unwrap_or_else(|| "Review the current changes".to_string());
			let checks = payload.checks.unwrap_or_default();
			let status = ctx.shell_with_options(
				"git status --short || true",
				ShellOptions::new().timeout(Duration::from_secs(5)),
			)?;
			let diff_stat = ctx.shell_with_options(
				"git diff --stat || true",
				ShellOptions::new().timeout(Duration::from_secs(5)),
			)?;
			Ok(json!({
				"id": ctx.id(),
				"prompt": prompt,
				"inspect": {
					"gitStatusBefore": payload.git_status_before.unwrap_or_default(),
					"gitDiffStat": payload.git_diff_stat.unwrap_or_default(),
					"gitChangedFiles": payload.git_changed_files.unwrap_or_default(),
					"rootFiles": payload.root_files.unwrap_or_default(),
					"projectFiles": payload.project_files.unwrap_or_default(),
					"status": status.stdout,
					"diffStat": diff_stat.stdout
				},
				"checks": checks,
				"findings": [],
				"risks": [],
				"summary": "Code-review template inspected repository state. Add model-backed finding generation next."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_test_fixer_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError, ShellOptions};
use serde::Deserialize;
use serde_json::json;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct TestFixerPayload {
	#[serde(rename = "failingCheck")]
	failing_check: Option<String>,
	#[serde(rename = "failureOutput")]
	failure_output: Option<String>,
	prompt: Option<String>,
	checks: Option<Vec<String>>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("test-fixer", |ctx: AgentContext| {
			let payload: TestFixerPayload = ctx.payload()?;
			let failing_check = payload
				.failing_check
				.unwrap_or_else(|| "cargo test".to_string());
			let failure_output = payload.failure_output.unwrap_or_default();
			let failure_excerpt = failure_output.chars().take(1200).collect::<String>();
			let status = ctx.shell_with_options(
				"git status --short || true",
				ShellOptions::new().timeout(Duration::from_secs(5)),
			)?;
			Ok(json!({
				"id": ctx.id(),
				"prompt": payload.prompt.unwrap_or_else(|| "Fix the failing test".to_string()),
				"failingCheck": failing_check,
				"failureExcerpt": failure_excerpt,
				"workspaceStatus": status.stdout,
				"checks": payload.checks.unwrap_or_default(),
				"repairPlan": [
					"Read the failing output and identify the first deterministic failure.",
					"Inspect the smallest related source and test files.",
					"Apply a focused fix, then rerun the failing check."
				],
				"summary": "Test-fixer template prepared a repair loop scaffold. Add model-backed patch generation next."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_repo_analyst_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct RepoAnalystPayload {
	question: Option<String>,
	paths: Option<Vec<String>>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("repo-analyst", |ctx: AgentContext| {
			let payload: RepoAnalystPayload = ctx.payload()?;
			let question = payload
				.question
				.unwrap_or_else(|| "Map this repository".to_string());
			let root_files = ctx.readdir(".").unwrap_or_default();
			let rust_sources = ctx.glob("src/*.rs").unwrap_or_default();
			Ok(json!({
				"id": ctx.id(),
				"question": question,
				"requestedPaths": payload.paths.unwrap_or_default(),
				"rootFiles": root_files,
				"rustSources": rust_sources,
				"entrypoints": rust_sources,
				"summary": "Repo-analyst template inventoried the workspace. Add model-backed architecture synthesis next."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

fn scaffold_support_main_rs() -> &'static str {
    r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct SupportPayload {
	message: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new()
		.with_workspace(".")
		.load_workspace_context()?
		.agent(AgentDefinition::webhook("support", |ctx: AgentContext| {
			let payload: SupportPayload = ctx.payload()?;
			let message = payload.message.unwrap_or_else(|| "Help requested".to_string());
			let role = ctx
				.role("reviewer")
				.map(|role| role.description.clone())
				.unwrap_or_else(|| "support reviewer".to_string());
			Ok(json!({
				"id": ctx.id(),
				"message": message,
				"role": role,
				"answer": "Support template executed. Add a knowledge-base SessionEnv or model-backed skill next."
			}))
		})))
}

fn main() {
	let code = match app().and_then(run_cli) {
		Ok(code) => code,
		Err(err) => {
			eprintln!("[agentic-harness] {err}");
			1
		}
	};
	std::process::exit(code);
}
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_snapshot_detects_file_add_modify_and_delete() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let main = src.join("main.rs");
        fs::write(&main, "fn main() {}\n").unwrap();

        let initial = WatchSnapshot::read(temp.path()).unwrap();
        assert!(!initial.has_changed(&WatchSnapshot::read(temp.path()).unwrap()));

        fs::write(&main, "fn main() { println!(\"changed\"); }\n").unwrap();
        assert!(initial.has_changed(&WatchSnapshot::read(temp.path()).unwrap()));

        let modified = WatchSnapshot::read(temp.path()).unwrap();
        fs::write(src.join("lib.rs"), "pub fn lib() {}\n").unwrap();
        assert!(modified.has_changed(&WatchSnapshot::read(temp.path()).unwrap()));

        let added = WatchSnapshot::read(temp.path()).unwrap();
        fs::remove_file(&main).unwrap();
        assert!(added.has_changed(&WatchSnapshot::read(temp.path()).unwrap()));
    }

    #[test]
    fn watch_snapshot_ignores_generated_and_vcs_directories() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("src")).unwrap();
        fs::create_dir_all(temp.path().join("target/debug")).unwrap();
        fs::create_dir_all(temp.path().join(".git")).unwrap();
        fs::write(temp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(temp.path().join("target/debug/build.log"), "one\n").unwrap();
        fs::write(temp.path().join(".git/index"), "one\n").unwrap();

        let initial = WatchSnapshot::read(temp.path()).unwrap();
        fs::write(temp.path().join("target/debug/build.log"), "two\n").unwrap();
        fs::write(temp.path().join(".git/index"), "two\n").unwrap();

        assert!(!initial.has_changed(&WatchSnapshot::read(temp.path()).unwrap()));
    }

    #[test]
    fn status_probe_allows_slow_cli_startup() {
        let temp = tempfile::tempdir().unwrap();
        let probe = temp.path().join("slow-probe");
        fs::write(
            &probe,
            "#!/bin/sh\nsleep 3\nprintf 'Logged in using ChatGPT\\n'\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&probe).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&probe, permissions).unwrap();

        let output = run_status_probe(probe.to_str().unwrap(), &["login", "status"]).unwrap();

        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("Logged in"));
    }

    #[test]
    fn add_mode_detects_common_coding_agent_environments() {
        assert!(should_print_agent_instructions_for(
            false,
            true,
            false,
            [("CODEX_SANDBOX", "workspace-write")]
        ));
        assert!(should_print_agent_instructions_for(
            false,
            true,
            false,
            [("CLAUDECODE", "1")]
        ));
        assert!(should_print_agent_instructions_for(
            false,
            true,
            false,
            [("CURSOR_AGENT", "1")]
        ));
        assert!(should_print_agent_instructions_for(
            false,
            true,
            false,
            [("OPENCODE", "1")]
        ));
    }

    #[test]
    fn add_mode_keeps_human_hint_when_terminal_and_no_agent_signal() {
        assert!(!should_print_agent_instructions_for(
            false,
            true,
            false,
            std::iter::empty::<(&str, &str)>()
        ));
    }

    #[test]
    fn add_mode_human_override_beats_agent_detection() {
        assert!(!should_print_agent_instructions_for(
            false,
            true,
            true,
            [("CODEX_SANDBOX", "workspace-write")]
        ));
    }

    #[test]
    fn plain_wizard_dashboard_has_no_ansi_escape_codes() {
        let plain = wizard_dashboard(Path::new("."));

        assert!(!plain.contains("\x1b["));
        assert!(plain.contains("1. Start coding"));
        assert!(plain.contains("1. Start coding here"));
        assert!(plain.contains("2. Start coding with detected LLM"));
        assert!(plain.contains("agentic-harness code --workspace . --llm auto"));
        assert!(plain.contains("Pick a workflow number first"));
        assert!(plain.contains("1. Create template pack"));
        assert!(plain.contains("2. Create with LLM"));
        assert!(plain.contains("3. List templates"));
        assert!(plain.contains("4. Search templates"));
        assert!(plain.contains("agentic-harness templates search review"));
        assert!(plain.contains("1. Claude Code"));
        assert!(plain.contains("1. Local checkout"));
        assert!(plain.contains("1. Inspect latest result"));
        assert!(plain.contains("3. Open dashboard"));
        assert!(plain.contains("5. Check current workspace"));
        assert!(plain.contains("2. Run from your checkout"));
        assert!(!plain.contains("Host with"));
        assert!(!plain.contains("InsForge"));
        assert!(!plain.contains("Cloudflare"));
    }

    #[test]
    fn styled_wizard_dashboard_uses_clack_style_color_and_markers() {
        let styled = styled_wizard_dashboard(Path::new("."));

        assert!(styled.contains("\x1b["));
        assert!(styled.contains("◇"));
        assert!(styled.contains("│"));
        assert!(styled.contains("Agentic Harness Wizard"));
        assert!(styled.contains("Status panel"));
        assert!(styled.contains("Readiness"));
        assert!(styled.contains("Sandbox"));
        assert!(styled.contains("Templates"));
        assert!(styled.contains("Pick a workflow number first"));
        assert!(styled.contains("Start coding"));
        assert!(!styled.contains("Host with"));
    }

    #[test]
    fn wizard_workflows_expose_multiple_selectable_options() {
        assert_eq!(WIZARD_STEPS.len(), 6);
        assert!(WIZARD_STEPS.iter().all(|step| step.choices.len() >= 3));
        assert!(find_wizard_step("1")
            .and_then(|step| find_wizard_choice(step, "1"))
            .is_some_and(|choice| choice.action == Some(WizardAction::CodeCurrent)));
        assert!(find_wizard_step("1")
            .and_then(|step| find_wizard_choice(step, "2"))
            .is_some_and(|choice| choice.action == Some(WizardAction::CodeCurrentWithAutoLlm)));
        assert!(find_wizard_step("2")
            .and_then(|step| find_wizard_choice(step, "1"))
            .is_some_and(|choice| choice.action == Some(WizardAction::TemplateInit)));
        assert!(find_wizard_step("2")
            .and_then(|step| find_wizard_choice(step, "2"))
            .is_some_and(|choice| choice.action == Some(WizardAction::TemplateAuthor)));
        assert!(find_wizard_step("2")
            .and_then(|step| find_wizard_choice(step, "3"))
            .is_some_and(|choice| choice.action == Some(WizardAction::TemplateList)));
        assert!(find_wizard_step("2")
            .and_then(|step| find_wizard_choice(step, "4"))
            .is_some_and(|choice| choice.action == Some(WizardAction::TemplateSearch)));
        assert!(find_wizard_step("3")
            .and_then(|step| find_wizard_choice(step, "1"))
            .is_some_and(|choice| choice.action
                == Some(WizardAction::SetupLlm(LlmAuthoringEnvironment::ClaudeCode))));
        assert!(find_wizard_step("3")
            .and_then(|step| find_wizard_choice(step, "5"))
            .is_some_and(|choice| choice.action == Some(WizardAction::SetupLlmAuto)));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "2"))
            .is_some_and(|choice| choice.action == Some(WizardAction::SandboxStatus)));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "3"))
            .is_some_and(|choice| choice.action == Some(WizardAction::SandboxExec)));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "4"))
            .is_some_and(|choice| choice.action == Some(WizardAction::SandboxSync)));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "5"))
            .is_some_and(|choice| choice.action == Some(WizardAction::SandboxLogs)));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "6"))
            .is_some_and(|choice| choice.action == Some(WizardAction::SandboxCleanup)));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "7"))
            .is_some_and(|choice| choice.title == "Vercel Sandbox"));
        assert!(find_wizard_step("4")
            .and_then(|step| find_wizard_choice(step, "9"))
            .is_some_and(|choice| choice.action
                == Some(WizardAction::SetupSandboxRemote(SandboxTarget::E2b))));
        assert!(find_wizard_step("6")
            .and_then(|step| find_wizard_choice(step, "1"))
            .is_some_and(|choice| choice.action == Some(WizardAction::ResultCurrent)));
        assert!(find_wizard_step("6")
            .and_then(|step| find_wizard_choice(step, "2"))
            .is_some_and(|choice| choice.action == Some(WizardAction::ResultCustom)));
        assert!(find_wizard_step("6")
            .and_then(|step| find_wizard_choice(step, "3"))
            .is_some_and(|choice| choice.action == Some(WizardAction::DashboardCurrent)));
        assert!(find_wizard_step("6")
            .and_then(|step| find_wizard_choice(step, "4"))
            .is_some_and(|choice| choice.action == Some(WizardAction::DashboardCustom)));
        assert!(find_wizard_step("6")
            .and_then(|step| find_wizard_choice(step, "6"))
            .is_some_and(|choice| choice.action == Some(WizardAction::DoctorExample)));
    }

    #[test]
    fn wizard_template_registry_actions_show_scope_selection() {
        let template_step = find_wizard_step("2").unwrap();
        let install = find_wizard_choice(template_step, "8").unwrap();
        let import = find_wizard_choice(template_step, "9").unwrap();

        assert!(install.detail.contains("workspace, user, or team"));
        assert!(install.command.contains("--scope"));
        assert!(import.detail.contains("workspace, user, or team"));
        assert!(import.command.contains("--scope"));
    }

    #[test]
    fn wizard_choices_are_concrete_or_prompt_driven_without_placeholders() {
        for step in WIZARD_STEPS.iter() {
            for choice in step.choices {
                assert!(
                    choice.action.is_some(),
                    "{} / {} must have an executable wizard action",
                    step.title,
                    choice.title
                );
                assert!(
                    !choice.command.contains('<') && !choice.command.contains('>'),
                    "{} / {} still shows an unusable placeholder command: {}",
                    step.title,
                    choice.title,
                    choice.command
                );
            }
        }
    }

    #[test]
    fn styled_wizard_options_show_navigation_and_nested_choices() {
        let step = find_wizard_step("4").unwrap();
        let styled = styled_wizard_options(step, Path::new("."));

        assert!(styled.contains("\x1b["));
        assert!(styled.contains("Set up sandbox"));
        assert!(styled.contains("Local checkout"));
        assert!(styled.contains("Vercel Sandbox"));
        assert!(styled.contains("E2B"));
        assert!(styled.contains("b back"));
        assert!(styled.contains("q quit"));
    }

    #[test]
    fn wizard_selection_options_show_workspace_aware_commands_before_selection() {
        let workspace = Path::new("/tmp/agentic-harness-tui-agent");
        let styled = styled_wizard_options(find_wizard_step("1").unwrap(), workspace);

        assert!(styled.contains("Start coding here"));
        assert!(styled.contains("agentic-harness code --workspace /tmp/agentic-harness-tui-agent"));
        assert!(styled.contains("Start coding with detected LLM"));
        assert!(styled.contains(
            "agentic-harness code --workspace /tmp/agentic-harness-tui-agent --llm auto"
        ));
        assert!(styled.contains("Start coding in another workspace"));
        assert!(styled.contains("prompts for workspace and prompt"));
    }

    #[test]
    fn wizard_selection_screens_include_contextual_status_panels() {
        let workspace = Path::new("/tmp/agentic-harness-panel-agent");
        let coding = styled_wizard_options(find_wizard_step("1").unwrap(), workspace);
        assert!(coding.contains("Coding panel"));
        assert!(coding.contains("LLM tools"));
        assert!(coding.contains("Latest result"));
        assert!(
            coding.contains("agentic-harness inspect --workspace /tmp/agentic-harness-panel-agent")
        );

        let templates = styled_wizard_options(find_wizard_step("2").unwrap(), Path::new("."));
        assert!(templates.contains("Template panel"));
        assert!(templates.contains("8 built-in"));
        assert!(templates.contains("agentic-harness templates ls --workspace . --json"));
        assert!(templates.contains("agentic-harness tpl preview coding --workspace . --json"));
        assert!(templates.contains("Preview picks"));
        assert!(templates.contains("coding"));
        assert!(templates.contains("Repository coding agent"));
        assert!(templates.contains("code-review"));
        assert!(templates.contains("Repository review agent"));
        assert!(templates.contains("test-fixer"));
        assert!(templates.contains("Focused test repair agent"));
        assert!(templates.contains("agentic-harness new ./my-agent --template coding"));

        let llm = styled_wizard_options(find_wizard_step("3").unwrap(), workspace);
        assert!(llm.contains("LLM panel"));
        assert!(llm.contains("Authoring context"));
        assert!(llm.contains("Auto detection"));
        assert!(llm.contains(
            "agentic-harness setup llm --workspace /tmp/agentic-harness-panel-agent --env auto"
        ));

        let sandbox = styled_wizard_options(find_wizard_step("4").unwrap(), workspace);
        assert!(sandbox.contains("Sandbox panel"));
        assert!(sandbox.contains("Target"));
        assert!(sandbox.contains("Recent logs"));
        assert!(sandbox.contains(
            "agentic-harness sandbox status --workspace /tmp/agentic-harness-panel-agent --json"
        ));
        assert!(sandbox.contains(
            "agentic-harness sandbox logs --workspace /tmp/agentic-harness-panel-agent --json"
        ));

        let run = styled_wizard_options(find_wizard_step("5").unwrap(), Path::new("."));
        assert!(run.contains("Run panel"));
        assert!(run.contains("Example workspace"));
        assert!(run.contains("Manifest"));
        assert!(run.contains("agentic-harness manifest --workspace"));

        let check = styled_wizard_options(find_wizard_step("6").unwrap(), workspace);
        assert!(check.contains("Check panel"));
        assert!(check.contains("Readiness"));
        assert!(check.contains("Next fix"));
        assert!(check.contains(
            "agentic-harness doctor --workspace /tmp/agentic-harness-panel-agent --json"
        ));
    }

    #[test]
    fn coding_wizard_panel_shows_latest_loop_status() {
        let temp = tempfile::tempdir().unwrap();
        let runs = temp.path().join(".agentic-harness/runs");
        fs::create_dir_all(&runs).unwrap();
        fs::write(
            runs.join("latest.json"),
            serde_json::json!({
                "loop": [
                    {"phase": "inspect", "status": "completed"},
                    {"phase": "edit", "status": "applied"},
                    {"phase": "test", "status": "passed"}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let panel = render_coding_wizard_panel(temp.path(), false);

        assert!(panel.contains("Latest loop"));
        assert!(panel.contains("inspect: completed"));
        assert!(panel.contains("edit: applied"));
        assert!(panel.contains("test: passed"));
    }

    #[test]
    fn sandbox_wizard_panel_shows_smoke_status_and_recent_log() {
        let temp = tempfile::tempdir().unwrap();
        append_sandbox_log(temp.path(), "write notes/seed.txt").unwrap();

        let panel = render_sandbox_wizard_panel(temp.path(), false);

        assert!(panel.contains("Smoke: ok - local checkout smoke ok"));
        assert!(panel.contains("Recent log: write notes/seed.txt"));
        assert!(panel.contains(&format!(
            "agentic-harness sandbox status --workspace {} --json",
            temp.path().display()
        )));
    }

    #[test]
    fn cursor_llm_invocation_uses_headless_print_mode() {
        let invocation = llm_prompt_invocation(
            LlmAuthoringEnvironment::Cursor,
            "Create a template",
            Path::new("brief.md"),
        )
        .unwrap();

        assert_eq!(invocation.command, "cursor");
        assert_eq!(
            invocation.args,
            vec![
                "agent",
                "--print",
                "--trust",
                "--force",
                "Create a template"
            ]
        );
        assert!(invocation.stdin.is_none());
    }

    #[test]
    fn sha256_hex_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"agentic-harness"),
            "8c7ccf717e5aa0e7fa9d9c96a84a8f10cb54e6d507e73d64a7a681614317f428"
        );
    }
}
