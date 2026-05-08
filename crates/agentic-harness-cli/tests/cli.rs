use serde_json::json;
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

fn agentic_harness_bin() -> &'static str {
    env!("CARGO_BIN_EXE_agentic-harness")
}

fn collect_previous_name_offenders(
    repo_root: &Path,
    path: &Path,
    banned: &[&String],
    offenders: &mut Vec<String>,
) {
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    if relative
        .components()
        .any(|component| matches!(component.as_os_str().to_str(), Some(".git" | "target")))
        || relative.starts_with("examples/hello-world/.agentic-harness/runs")
        || relative == Path::new(&["docs/", "fl", "ue", "-migration.md"].concat())
    {
        return;
    }

    let relative_text = relative.display().to_string();
    if banned
        .iter()
        .any(|token| relative_text.contains(token.as_str()))
    {
        offenders.push(relative_text);
        return;
    }

    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        let Ok(entries) = fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            collect_previous_name_offenders(repo_root, &entry.path(), banned, offenders);
        }
        return;
    }

    if metadata.is_file() {
        let Ok(content) = fs::read_to_string(path) else {
            return;
        };
        if banned.iter().any(|token| content.contains(token.as_str())) {
            offenders.push(relative_text);
        }
    }
}

fn copy_test_workspace(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let name = entry.file_name();
        if matches!(name.to_str(), Some("target" | ".git")) {
            continue;
        }
        let destination_path = destination.join(name);
        if entry.file_type().unwrap().is_dir() {
            copy_test_workspace(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).unwrap();
        }
    }
}

fn read_tar_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let bytes = fs::read(path).unwrap();
    let mut entries = Vec::new();
    let mut offset = 0;
    while offset + 512 <= bytes.len() {
        let header = &bytes[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name_len = header[..100]
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(100);
        let name = String::from_utf8_lossy(&header[..name_len]).to_string();
        let size_len = header[124..136]
            .iter()
            .position(|byte| *byte == 0 || *byte == b' ')
            .unwrap_or(12);
        let size_text = String::from_utf8_lossy(&header[124..124 + size_len]);
        let size = usize::from_str_radix(size_text.trim(), 8).unwrap();
        offset += 512;
        entries.push((name, bytes[offset..offset + size].to_vec()));
        offset += size.div_ceil(512) * 512;
    }
    entries
}

fn write_latest_coding_run(project: &Path) {
    let runs = project.join(".agentic-harness/runs");
    fs::create_dir_all(&runs).unwrap();
    fs::write(
        runs.join("latest.json"),
        serde_json::to_string_pretty(&json!({
            "loop": [
                {"phase": "inspect", "status": "completed", "detail": "workspace inspected"},
                {"phase": "plan", "status": "completed", "detail": "4 planned steps"},
                {"phase": "edit", "status": "applied", "detail": "1 patch, 1 changed file"},
                {"phase": "test", "status": "passed", "detail": "1/1 check passed"},
                {"phase": "summarize", "status": "completed", "detail": "1 changed file recorded"},
                {"phase": "commit", "status": "skipped", "detail": "no commit requested"},
                {"phase": "pull-request", "status": "skipped", "detail": "no pull request requested"}
            ],
            "changedFiles": ["src/main.rs"],
            "checks": [
                {"command": "cargo test", "status": "passed", "success": true}
            ]
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn new_scaffolds_a_native_rust_agent_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("demo-agent");

    let output = Command::new(agentic_harness_bin())
        .args(["new", project.to_str().unwrap(), "--name", "demo-agent"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project.join("Cargo.toml").exists());
    assert!(project.join("src/main.rs").exists());
    assert!(project.join("AGENTS.md").exists());
    assert!(project.join(".agentic-harness/roles/greeter.md").exists());

    let cargo_toml = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("name = \"demo-agent\""));
    assert!(cargo_toml.contains("agentic-harness = { path = "));
}

#[test]
fn new_scaffolds_source_style_agent_templates() {
    let temp = tempfile::tempdir().unwrap();
    let cases = [
        ("triage", "triage"),
        ("data", "data"),
        ("coding", "code"),
        ("code-review", "code-review"),
        ("test-fixer", "test-fixer"),
        ("docs-writer", "docs-writer"),
        ("refactor-agent", "refactor-agent"),
        ("release-agent", "release-agent"),
        ("repo-analyst", "repo-analyst"),
        ("support", "support"),
    ];

    for (template, agent_name) in cases {
        let project = temp.path().join(format!("{template}-agent"));
        let output = Command::new(agentic_harness_bin())
            .args([
                "new",
                project.to_str().unwrap(),
                "--name",
                &format!("{template}-agent"),
                "--template",
                template,
            ])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "template {template} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let main_rs = fs::read_to_string(project.join("src/main.rs")).unwrap();
        assert!(
            main_rs.contains(&format!("AgentDefinition::webhook(\"{agent_name}\""))
                || main_rs.contains(&format!("AgentDefinition::cli_only(\"{agent_name}\"")),
            "template {template} did not register expected agent {agent_name}"
        );
        assert!(project.join("AGENTS.md").exists());
        assert!(project.join(".agents/skills").exists());
        assert!(project.join(".agentic-harness/roles").exists());
    }
}

fn write_review_template(path: &Path) {
    fs::create_dir_all(path.join("files/src")).unwrap();
    fs::create_dir_all(path.join("files/.agentic-harness/roles")).unwrap();
    fs::create_dir_all(path.join("files/.agents/skills/review")).unwrap();
    fs::create_dir_all(path.join("examples")).unwrap();
    fs::write(
        path.join("agentic-template.toml"),
        r#"name = "review"
agent = "review"
description = "Repository review agent"
sample_payload = "{\"prompt\":\"Review the current diff\"}"
"#,
    )
    .unwrap();
    fs::write(
        path.join("files/Cargo.toml.hbs"),
        r#"[package]
name = "{{package_name}}"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = { path = "{{crate_path}}" }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
"#,
    )
    .unwrap();
    fs::write(
        path.join("files/src/main.rs.hbs"),
        r#"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde::Deserialize;
use serde_json::json;

#[derive(Debug, Deserialize)]
struct ReviewPayload {
	prompt: Option<String>,
}

fn app() -> Result<AgentApp, AgenticHarnessError> {
	Ok(AgentApp::new().agent(AgentDefinition::webhook("{{agent_name}}", |ctx: AgentContext| {
		let payload: ReviewPayload = ctx.payload()?;
		Ok(json!({
			"id": ctx.id(),
			"prompt": payload.prompt.unwrap_or_else(|| "Review the repo".to_string()),
			"template": "{{template_name}}"
		}))
	})))
}

fn main() {
	std::process::exit(app().and_then(run_cli).unwrap_or(1));
}
"#,
    )
    .unwrap();
    fs::write(
        path.join("files/AGENTS.md.hbs"),
        "You are a {{template_name}} agent for {{package_name}}.\n",
    )
    .unwrap();
    fs::write(
        path.join("files/.agentic-harness/roles/reviewer.md"),
        "---\ndescription: Repository reviewer\n---\nReview risks and tests.\n",
    )
    .unwrap();
    fs::write(
        path.join("files/.agents/skills/review/SKILL.md"),
        "---\nname: review\ndescription: Review a repository change\n---\nInspect files before summarizing.\n",
    )
    .unwrap();
    fs::write(
        path.join("examples/payload.json"),
        r#"{"prompt":"Review the current diff"}"#,
    )
    .unwrap();
}

#[test]
fn template_pack_can_validate_install_and_scaffold_project() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("review-template");
    let project = temp.path().join("review-agent");
    write_review_template(&template);

    let validate = Command::new(agentic_harness_bin())
        .args(["template", "validate", template.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    assert!(String::from_utf8_lossy(&validate.stdout).contains("template: ok"));

    let install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    assert!(temp
        .path()
        .join(".agentic-harness/templates/review/agentic-template.toml")
        .exists());

    let installed_preview = Command::new(agentic_harness_bin())
        .args([
            "template",
            "show",
            "review",
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        installed_preview.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&installed_preview.stderr)
    );
    let preview_body = String::from_utf8_lossy(&installed_preview.stdout);
    assert!(preview_body.contains("Template: review"));
    assert!(preview_body.contains("Source: installed"));
    assert!(preview_body.contains("Agent: review"));
    assert!(preview_body.contains("Repository review agent"));
    assert!(preview_body.contains("files/src/main.rs.hbs"));

    let builtin_preview = Command::new(agentic_harness_bin())
        .args(["template", "show", "coding"])
        .output()
        .unwrap();
    assert!(builtin_preview.status.success());
    let builtin_body = String::from_utf8_lossy(&builtin_preview.stdout);
    assert!(builtin_body.contains("Template: coding"));
    assert!(builtin_body.contains("Source: built-in"));
    assert!(builtin_body.contains("Agent: code"));

    let scaffold = Command::new(agentic_harness_bin())
        .current_dir(temp.path())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "review-agent",
            "--template",
            "review",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    assert!(fs::read_to_string(project.join("src/main.rs"))
        .unwrap()
        .contains("AgentDefinition::webhook(\"review\""));
    assert!(fs::read_to_string(project.join("AGENTS.md"))
        .unwrap()
        .contains("review agent for review-agent"));
}

#[test]
fn template_registry_can_export_and_import_template_packs() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("review-template");
    let exported = temp.path().join("exported-review");
    let team_workspace = temp.path().join("team-workspace");
    write_review_template(&template);

    let install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let export = Command::new(agentic_harness_bin())
        .args([
            "template",
            "export",
            "review",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--output",
            exported.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        export.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(exported.join("agentic-template.toml").exists());
    assert!(exported.join("files/src/main.rs.hbs").exists());

    let import = Command::new(agentic_harness_bin())
        .args([
            "template",
            "import",
            exported.to_str().unwrap(),
            "--workspace",
            team_workspace.to_str().unwrap(),
            "--name",
            "review-copy",
        ])
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&import.stderr)
    );
    assert!(team_workspace
        .join(".agentic-harness/templates/review-copy/agentic-template.toml")
        .exists());

    let list = Command::new(agentic_harness_bin())
        .args([
            "template",
            "list",
            "--workspace",
            team_workspace.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(String::from_utf8_lossy(&list.stdout).contains("review-copy"));
}

#[test]
fn template_registry_supports_user_and_team_scopes() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("review-template");
    let project = temp.path().join("team-review-agent");
    write_review_template(&template);

    let user_install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
            "--scope",
            "user",
            "--name",
            "personal-review",
        ])
        .output()
        .unwrap();
    assert!(
        user_install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&user_install.stderr)
    );
    assert!(temp
        .path()
        .join(".agentic-harness/templates/user/personal-review/agentic-template.toml")
        .exists());

    let team_import = Command::new(agentic_harness_bin())
        .args([
            "template",
            "import",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
            "--scope",
            "team",
            "--name",
            "team-review",
        ])
        .output()
        .unwrap();
    assert!(
        team_import.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&team_import.stderr)
    );
    assert!(temp
        .path()
        .join(".agentic-harness/templates/team/team-review/agentic-template.toml")
        .exists());

    let list = Command::new(agentic_harness_bin())
        .args([
            "template",
            "list",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--verbose",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let body = String::from_utf8_lossy(&list.stdout);
    assert!(body.contains("User templates:"));
    assert!(body.contains("personal-review (agent review"));
    assert!(body.contains("Team templates:"));
    assert!(body.contains("team-review (agent review"));

    let preview = Command::new(agentic_harness_bin())
        .args([
            "template",
            "show",
            "personal-review",
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert!(String::from_utf8_lossy(&preview.stdout).contains("Source: user"));

    let scaffold = Command::new(agentic_harness_bin())
        .current_dir(temp.path())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "team-review-agent",
            "--template",
            "team-review",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    assert!(project.join("src/main.rs").exists());
}

#[test]
fn template_aliases_support_short_human_commands() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("alias-template");

    let create = Command::new(agentic_harness_bin())
        .args([
            "tpl",
            "create",
            template.to_str().unwrap(),
            "--name",
            "alias-review",
            "--agent",
            "alias-review",
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&create.stderr)
    );

    let check = Command::new(agentic_harness_bin())
        .args(["templates", "check", template.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    assert!(String::from_utf8_lossy(&check.stdout).contains("template: ok"));

    let add = Command::new(agentic_harness_bin())
        .args([
            "tpl",
            "add",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
            "--scope",
            "user",
        ])
        .output()
        .unwrap();
    assert!(
        add.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&add.stderr)
    );

    let list = Command::new(agentic_harness_bin())
        .args([
            "templates",
            "ls",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--verbose",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(String::from_utf8_lossy(&list.stdout).contains("alias-review"));

    let preview = Command::new(agentic_harness_bin())
        .args([
            "tpl",
            "preview",
            "alias-review",
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        preview.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&preview.stderr)
    );
    let body = String::from_utf8_lossy(&preview.stdout);
    assert!(body.contains("Template: alias-review"));
    assert!(body.contains("Source: user"));
}

#[test]
fn template_registry_lists_template_versions() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("versioned-template");

    let init = Command::new(agentic_harness_bin())
        .args([
            "template",
            "init",
            template.to_str().unwrap(),
            "--name",
            "bugfix",
            "--agent",
            "bugfix",
            "--description",
            "Bug fixing coding agent",
            "--version",
            "0.2.0",
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let validate = Command::new(agentic_harness_bin())
        .args(["template", "validate", template.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    assert!(String::from_utf8_lossy(&validate.stdout).contains("version: 0.2.0"));

    let install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let list = Command::new(agentic_harness_bin())
        .args([
            "template",
            "list",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--verbose",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let body = String::from_utf8_lossy(&list.stdout);
    assert!(body.contains("bugfix"));
    assert!(body.contains("version 0.2.0"));
    assert!(body.contains("agent bugfix"));
}

#[test]
fn template_registry_lists_machine_readable_json() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("json-template");

    let init = Command::new(agentic_harness_bin())
        .args([
            "template",
            "init",
            template.to_str().unwrap(),
            "--name",
            "json-review",
            "--agent",
            "json-review",
            "--description",
            "JSON-visible review template",
            "--version",
            "0.4.1",
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let install = Command::new(agentic_harness_bin())
        .args([
            "tpl",
            "add",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
            "--scope",
            "team",
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let list = Command::new(agentic_harness_bin())
        .args([
            "templates",
            "ls",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert!(body["workspace"].as_str().unwrap().contains(".tmp"));
    assert_eq!(body["counts"]["builtIn"], 11);
    assert_eq!(body["counts"]["team"], 1);
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "coding"
            && template["source"] == "built-in"
            && template["valid"] == true));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "code-review"
            && template["source"] == "built-in"
            && template["description"].as_str().unwrap().contains("review")));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "test-fixer"
            && template["source"] == "built-in"
            && template["agent"] == "test-fixer"));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "docs-writer"
            && template["source"] == "built-in"
            && template["agent"] == "docs-writer"));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "refactor-agent"
            && template["source"] == "built-in"
            && template["agent"] == "refactor-agent"));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "release-agent"
            && template["source"] == "built-in"
            && template["agent"] == "release-agent"));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "repo-analyst"
            && template["source"] == "built-in"
            && template["valid"] == true));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "json-review"
            && template["source"] == "team"
            && template["version"] == "0.4.1"
            && template["description"] == "JSON-visible review template"));
}

#[test]
fn template_registry_searches_builtins_and_installed_templates() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("search-template");

    let init = Command::new(agentic_harness_bin())
        .args([
            "template",
            "init",
            template.to_str().unwrap(),
            "--name",
            "review-helper",
            "--agent",
            "review-helper",
            "--description",
            "Team review checklist template",
            "--version",
            "0.5.0",
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
            "--scope",
            "team",
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let plain = Command::new(agentic_harness_bin())
        .args([
            "tpl",
            "find",
            "review",
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        plain.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&plain.stderr)
    );
    let body = String::from_utf8_lossy(&plain.stdout);
    assert!(body.contains("Template search: review"));
    assert!(body.contains("code-review (built-in"));
    assert!(body.contains("review-helper (team"));
    assert!(!body.contains("support (built-in"));

    let json = Command::new(agentic_harness_bin())
        .args([
            "templates",
            "search",
            "review",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["query"], "review");
    assert!(body["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "code-review" && template["source"] == "built-in"));
    assert!(body["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "review-helper"
            && template["source"] == "team"
            && template["version"] == "0.5.0"));
    assert!(!body["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "support"));
}

#[test]
fn template_preview_can_print_machine_readable_json() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("preview-template");

    let init = Command::new(agentic_harness_bin())
        .args([
            "template",
            "init",
            template.to_str().unwrap(),
            "--name",
            "preview-review",
            "--agent",
            "preview-agent",
            "--description",
            "Preview JSON template",
            "--version",
            "1.2.3",
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
            "--scope",
            "team",
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let built_in = Command::new(agentic_harness_bin())
        .args([
            "template",
            "show",
            "coding",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        built_in.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&built_in.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&built_in.stdout).unwrap();
    assert_eq!(body["name"], "coding");
    assert_eq!(body["source"], "built-in");
    assert_eq!(body["valid"], true);
    assert!(body["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "src/main.rs"));
    assert_eq!(
        body["nextCommand"],
        "agentic-harness new ./my-agent --template coding"
    );

    let installed = Command::new(agentic_harness_bin())
        .args([
            "templates",
            "preview",
            "preview-review",
            "--workspace",
            temp.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(body["name"], "preview-review");
    assert_eq!(body["source"], "team");
    assert_eq!(body["agent"], "preview-agent");
    assert_eq!(body["version"], "1.2.3");
    assert_eq!(body["description"], "Preview JSON template");
    assert_eq!(body["valid"], true);
    assert_eq!(body["fileCount"], body["files"].as_array().unwrap().len());
    assert!(body["path"].as_str().unwrap().contains("preview-review"));
    assert!(body["files"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "files/Cargo.toml.hbs"));
}

#[test]
fn template_init_creates_valid_pack_that_can_install_and_scaffold() {
    let temp = tempfile::tempdir().unwrap();
    let template = temp.path().join("created-template");
    let project = temp.path().join("created-agent");

    let init = Command::new(agentic_harness_bin())
        .args([
            "template",
            "init",
            template.to_str().unwrap(),
            "--name",
            "bugfix",
            "--agent",
            "bugfix",
            "--description",
            "Bug fixing coding agent",
        ])
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    assert!(template.join("agentic-template.toml").exists());
    assert!(template.join("files/Cargo.toml.hbs").exists());
    assert!(template.join("files/src/main.rs.hbs").exists());
    assert!(template.join("files/AGENTS.md.hbs").exists());
    assert!(template
        .join("files/.agents/skills/bugfix/SKILL.md")
        .exists());
    assert!(template.join("examples/payload.json").exists());

    let validate = Command::new(agentic_harness_bin())
        .args(["template", "validate", template.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );

    let install = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            temp.path().to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );

    let scaffold = Command::new(agentic_harness_bin())
        .current_dir(temp.path())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "created-agent",
            "--template",
            "bugfix",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    assert!(fs::read_to_string(project.join("src/main.rs"))
        .unwrap()
        .contains("AgentDefinition::webhook(\"bugfix\""));
}

#[test]
fn template_author_creates_llm_handoff_brief() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("author-workspace");
    fs::create_dir_all(&project).unwrap();

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let output = Command::new(agentic_harness_bin())
        .args([
            "template",
            "author",
            "bugfix",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create a repository bug fixing coding agent template",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("template brief:"));
    assert!(stdout.contains("codex"));
    assert!(stdout.contains("agentic-harness template validate ./bugfix"));
    assert!(stdout.contains("agentic-harness doctor --workspace ./bugfix-agent"));

    let brief_path = project.join(".agentic-harness/template-briefs/bugfix.md");
    let brief = fs::read_to_string(&brief_path).unwrap();
    assert!(brief.contains("# Agentic Harness Template Brief: bugfix"));
    assert!(brief.contains("Environment: Codex"));
    assert!(brief.contains("Create a repository bug fixing coding agent template"));
    assert!(brief.contains(".agents/skills/agentic-harness-template/SKILL.md"));
    assert!(brief.contains("agentic-template.toml"));
    assert!(brief.contains("Do not edit the Agentic Harness SDK or CLI crates"));
    assert!(brief.contains("If `agentic-harness` is not on PATH"));
    assert!(brief.contains(agentic_harness_bin()));
    assert!(brief.contains("agentic-harness template validate ./bugfix"));
    assert!(brief.contains("agentic-harness template preview ./bugfix --json"));
    assert!(brief.contains("agentic-harness template install ./bugfix"));
    assert!(brief.contains("agentic-harness new ./bugfix-agent --template bugfix"));

    let json = Command::new(agentic_harness_bin())
        .args([
            "template",
            "author",
            "bugfix-json",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create a JSON-reporting bugfix template",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["template"], "bugfix-json");
    assert_eq!(body["environment"], "codex");
    assert_eq!(body["prompt"], "Create a JSON-reporting bugfix template");
    assert!(body["briefPath"]
        .as_str()
        .unwrap()
        .ends_with(".agentic-harness/template-briefs/bugfix-json.md"));
    assert!(body["openCommand"]
        .as_str()
        .unwrap()
        .contains("codex exec -"));
    assert_eq!(
        body["validationCommands"][0],
        "agentic-harness template validate ./bugfix-json"
    );
    assert_eq!(
        body["validationCommands"][1],
        "agentic-harness template preview ./bugfix-json --json"
    );
    assert_eq!(
        body["validationCommands"][2],
        "agentic-harness template install ./bugfix-json"
    );
    assert_eq!(
        body["validationCommands"][3],
        "agentic-harness new ./bugfix-json-agent --template bugfix-json"
    );
    assert_eq!(
        body["validationCommands"][4],
        "agentic-harness doctor --workspace ./bugfix-json-agent"
    );
    assert_eq!(
        body["nextCommands"][4],
        "agentic-harness doctor --workspace ./bugfix-json-agent"
    );
}

#[test]
fn template_author_open_launches_the_selected_llm_command() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("author-open-workspace");
    fs::create_dir_all(&project).unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let capture = temp.path().join("opened-brief.txt");
    let args_capture = temp.path().join("opened-args.txt");
    let cwd_capture = temp.path().join("opened-cwd.txt");
    let fake_codex = bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  printf 'Logged in using ChatGPT\n'
  exit 0
fi
printf '%s\n' "$@" > "$AGENTIC_HARNESS_OPEN_ARGS"
printf '%s\n' "$PWD" > "$AGENTIC_HARNESS_OPEN_CWD"
cat > "$AGENTIC_HARNESS_OPEN_CAPTURE"
grep -q "Create a code review template" "$AGENTIC_HARNESS_OPEN_CAPTURE" || exit 9
mkdir -p reviewer/files/src reviewer/examples
printf 'name = "reviewer"\nagent = "reviewer"\nversion = "0.1.0"\ndescription = "Review code changes"\nsample_payload = "{\"prompt\":\"Review this repo\"}"\n' > reviewer/agentic-template.toml
printf '[package]\nname = "{{package_name}}"\nversion = "0.1.0"\nedition = "2021"\n' > reviewer/files/Cargo.toml.hbs
printf 'fn main() {}\n' > reviewer/files/src/main.rs.hbs
printf 'Use the reviewer agent.\n' > reviewer/files/AGENTS.md.hbs
printf '{"prompt":"Review this repo"}\n' > reviewer/examples/payload.json
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_codex).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).unwrap();

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("AGENTIC_HARNESS_OPEN_CAPTURE", &capture)
        .env("AGENTIC_HARNESS_OPEN_ARGS", &args_capture)
        .env("AGENTIC_HARNESS_OPEN_CWD", &cwd_capture)
        .args([
            "template",
            "author",
            "reviewer",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create a code review template",
            "--open",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("opened with: codex"));
    assert!(stdout.contains("post-open validation: ok"));
    assert!(stdout.contains(&format!(
        "template path: {}",
        project.join("reviewer").display()
    )));
    assert_eq!(
        fs::read_to_string(args_capture).unwrap(),
        "exec\n--sandbox\nworkspace-write\n--skip-git-repo-check\n-\n"
    );
    assert_eq!(
        fs::read_to_string(cwd_capture).unwrap().trim(),
        project.canonicalize().unwrap().display().to_string()
    );
    let brief_path = project.join(".agentic-harness/template-briefs/reviewer.md");
    let opened = fs::read_to_string(capture).unwrap();
    assert!(opened.contains("# Agentic Harness Template Brief: reviewer"));
    assert!(opened.contains("Create a code review template"));
    assert!(brief_path.exists());

    let validate = Command::new(agentic_harness_bin())
        .args([
            "template",
            "validate",
            project.join("reviewer").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
}

#[test]
fn template_author_open_json_keeps_stdout_machine_readable() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("author-open-json");
    fs::create_dir_all(&project).unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let fake_codex = bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  printf 'Logged in using ChatGPT\n'
  exit 0
fi
cat >/dev/null
printf 'codex json stdout\n'
printf 'codex json stderr\n' >&2
mkdir -p reviewer/files/src reviewer/examples
printf 'name = "reviewer"\nagent = "reviewer"\nversion = "0.1.0"\ndescription = "Review code changes"\nsample_payload = "{\"prompt\":\"Review this repo\"}"\n' > reviewer/agentic-template.toml
printf '[package]\nname = "{{package_name}}"\nversion = "0.1.0"\nedition = "2021"\n' > reviewer/files/Cargo.toml.hbs
printf 'fn main() {}\n' > reviewer/files/src/main.rs.hbs
printf 'Use the reviewer agent.\n' > reviewer/files/AGENTS.md.hbs
printf '{"prompt":"Review this repo"}\n' > reviewer/examples/payload.json
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_codex).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).unwrap();

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "template",
            "author",
            "reviewer",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create a code review template",
            "--open",
            "--json",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["opened"], true);
    assert_eq!(body["postOpenValidation"]["ok"], true);
    assert_eq!(
        body["postOpenValidation"]["templatePath"],
        project.join("reviewer").display().to_string()
    );
    assert_eq!(
        body["postOpenValidation"]["nextCommand"],
        "agentic-harness template install ./reviewer"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.contains("codex json stdout"));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("codex json stdout"));
    assert!(stderr.contains("codex json stderr"));
}

#[test]
fn template_author_open_preflights_llm_readiness_before_launching() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("author-open-preflight");
    fs::create_dir_all(&project).unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let unexpected_launch = temp.path().join("unexpected-launch");
    let fake_claude = bin.join("claude");
    fs::write(
        &fake_claude,
        format!(
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '{{"loggedIn":false,"authMethod":"none"}}\n'
  exit 1
fi
touch "{}"
exit 0
"#,
            unexpected_launch.display()
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_claude).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_claude, permissions).unwrap();

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "claude-code",
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "template",
            "author",
            "reviewer",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "claude-code",
            "--prompt",
            "Create a code review template",
            "--open",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Claude Code is not ready"));
    assert!(stderr.contains("login required"));
    assert!(stderr.contains("agentic-harness doctor --json"));
    assert!(!unexpected_launch.exists());
    assert!(project
        .join(".agentic-harness/template-briefs/reviewer.md")
        .exists());
}

#[test]
fn template_author_open_streams_llm_output_before_process_exits() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("author-open-streaming");
    fs::create_dir_all(&project).unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    let capture = temp.path().join("opened-brief.txt");
    let live_marker = temp.path().join("live-output-written");
    let fake_codex = bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  printf 'Logged in using ChatGPT\n'
  exit 0
fi
cat > "$AGENTIC_HARNESS_OPEN_CAPTURE"
printf 'codex live stdout\n'
printf 'codex live stderr\n' >&2
: > "$AGENTIC_HARNESS_LIVE_MARKER"
sleep 2
mkdir -p reviewer/files/src reviewer/examples
printf 'name = "reviewer"\nagent = "reviewer"\nversion = "0.1.0"\ndescription = "Review code changes"\nsample_payload = "{\"prompt\":\"Review this repo\"}"\n' > reviewer/agentic-template.toml
printf '[package]\nname = "{{package_name}}"\nversion = "0.1.0"\nedition = "2021"\n' > reviewer/files/Cargo.toml.hbs
printf 'fn main() {}\n' > reviewer/files/src/main.rs.hbs
printf 'Use the reviewer agent.\n' > reviewer/files/AGENTS.md.hbs
printf '{"prompt":"Review this repo"}\n' > reviewer/examples/payload.json
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_codex).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).unwrap();

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let mut child = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("AGENTIC_HARNESS_OPEN_CAPTURE", &capture)
        .env("AGENTIC_HARNESS_LIVE_MARKER", &live_marker)
        .args([
            "template",
            "author",
            "reviewer",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create a code review template",
            "--open",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let marker_deadline = Instant::now() + Duration::from_secs(8);
    while !live_marker.exists() {
        if Instant::now() >= marker_deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("fake Codex did not reach live-output marker");
        }
        thread::sleep(Duration::from_millis(20));
    }

    let deadline = Instant::now() + Duration::from_millis(900);
    let mut streamed = Vec::new();
    let saw_live_output = loop {
        let now = Instant::now();
        if now >= deadline {
            break false;
        }
        match rx.recv_timeout(deadline.saturating_duration_since(now)) {
            Ok(line) if line.contains("codex live stdout") => break true,
            Ok(line) => streamed.push(line),
            Err(mpsc::RecvTimeoutError::Timeout) => break false,
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
    };
    if !saw_live_output {
        let _ = child.kill();
        let _ = child.wait();
        panic!("did not stream fake Codex stdout before exit; saw {streamed:?}");
    }

    let status = child.wait().unwrap();
    assert!(status.success());
    assert!(fs::read_to_string(capture)
        .unwrap()
        .contains("# Agentic Harness Template Brief: reviewer"));

    let validate = Command::new(agentic_harness_bin())
        .args([
            "template",
            "validate",
            project.join("reviewer").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
}

#[test]
fn setup_llm_installs_template_authoring_context_and_doctor_detects_it() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("authoring-agent");
    fs::create_dir_all(&project).unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(project
        .join(".agents/skills/agentic-harness-template/SKILL.md")
        .exists());
    assert!(project.join(".agentic-harness/llm-authoring.toml").exists());
    let example = project.join(".agentic-harness/template-examples/code-review");
    assert!(example.join("agentic-template.toml").exists());
    assert!(example.join("files/Cargo.toml.hbs").exists());
    assert!(example.join("files/src/main.rs.hbs").exists());
    assert!(example.join("examples/payload.json").exists());
    let skill =
        fs::read_to_string(project.join(".agents/skills/agentic-harness-template/SKILL.md"))
            .unwrap();
    assert!(skill.contains(".agentic-harness/template-examples/code-review"));

    let validate = Command::new(agentic_harness_bin())
        .args(["template", "validate", example.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&validate.stderr)
    );
    assert!(fs::read_to_string(project.join("AGENTS.md"))
        .unwrap()
        .contains("Agentic Harness Template Authoring"));

    let printed = Command::new(agentic_harness_bin())
        .args(["setup", "llm", "--env", "wind-server", "--print"])
        .output()
        .unwrap();
    assert!(printed.status.success());
    let printed_body = String::from_utf8_lossy(&printed.stdout);
    assert!(printed_body.contains("Wind Server"));
    assert!(printed_body.contains("agentic-template.toml"));

    let doctor = Command::new(agentic_harness_bin())
        .args([
            "doctor",
            "--workspace",
            project.to_str().unwrap(),
            "--plain",
        ])
        .output()
        .unwrap();
    let body = String::from_utf8_lossy(&doctor.stdout);
    assert!(body.contains("llm authoring: ok"));
}

#[test]
fn setup_sandbox_local_updates_doctor_and_remote_prints_instructions() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("sandbox-agent");
    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "sandbox-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(scaffold.status.success());

    let local = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(
        local.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&local.stderr)
    );
    assert!(project.join(".agentic-harness/sandbox.toml").exists());

    let doctor = Command::new(agentic_harness_bin())
        .args([
            "doctor",
            "--workspace",
            project.to_str().unwrap(),
            "--plain",
        ])
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&doctor.stdout).contains("sandbox: ok"));

    let remote = Command::new(agentic_harness_bin())
        .args(["setup", "sandbox", "--target", "vercel", "--print"])
        .output()
        .unwrap();
    assert!(remote.status.success());
    let body = String::from_utf8_lossy(&remote.stdout);
    assert!(body.contains("SessionEnv"));
    assert!(body.contains("Vercel Sandbox"));
    assert!(!body.contains("deploy"));

    let e2b = Command::new(agentic_harness_bin())
        .args(["setup", "sandbox", "--target", "e2b", "--print"])
        .output()
        .unwrap();
    assert!(
        e2b.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&e2b.stderr)
    );
    let body = String::from_utf8_lossy(&e2b.stdout);
    assert!(body.contains("Rust-native E2B sandbox connector"));
    assert!(body.contains("SessionEnv"));
    assert!(body.contains("ShellOptions::cwd"));
}

#[test]
fn local_hosting_setup_status_and_dashboard_json_expose_urls() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("hosted-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args(["new", project.to_str().unwrap(), "--name", "hosted-agent"])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "hosting",
            "--workspace",
            project.to_str().unwrap(),
            "--addr",
            "127.0.0.1:4777",
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );
    let config = fs::read_to_string(project.join(".agentic-harness/hosting.toml")).unwrap();
    assert!(config.contains("addr = \"127.0.0.1:4777\""));

    let status = Command::new(agentic_harness_bin())
        .args([
            "hosting",
            "status",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let status_json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["configured"], true);
    assert_eq!(status_json["addr"], "127.0.0.1:4777");
    assert_eq!(status_json["baseUrl"], "http://127.0.0.1:4777");
    assert_eq!(status_json["healthUrl"], "http://127.0.0.1:4777/health");
    assert_eq!(status_json["agentsUrl"], "http://127.0.0.1:4777/agents");
    assert_eq!(status_json["capabilities"]["serve"], true);
    assert_eq!(status_json["capabilities"]["devReload"], true);
    assert_eq!(status_json["capabilities"]["sse"], true);
    assert!(status_json["commands"]["start"]
        .as_str()
        .unwrap()
        .contains("agentic-harness host --workspace"));

    let dashboard = Command::new(agentic_harness_bin())
        .args([
            "dashboard",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        dashboard.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dashboard.stderr)
    );
    let dashboard_json: serde_json::Value = serde_json::from_slice(&dashboard.stdout).unwrap();
    assert_eq!(dashboard_json["localHosting"]["addr"], "127.0.0.1:4777");
    assert_eq!(
        dashboard_json["localHosting"]["baseUrl"],
        "http://127.0.0.1:4777"
    );

    let tui = Command::new(agentic_harness_bin())
        .args(["tui", "--workspace", project.to_str().unwrap(), "--plain"])
        .output()
        .unwrap();
    assert!(tui.status.success());
    let tui_body = String::from_utf8_lossy(&tui.stdout);
    assert!(tui_body.contains("Local hosting"));
    assert!(tui_body.contains("agentic-harness host --workspace"));
    assert!(tui_body.contains("agentic-harness hosting status --workspace"));
}

#[test]
fn sandbox_local_exec_read_write_and_list_work_from_cli() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("sandbox-ops");
    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "sandbox-ops",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(scaffold.status.success());

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(setup.status.success());

    let status = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "status",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(status.status.success());
    let body = String::from_utf8_lossy(&status.stdout);
    assert!(body.contains("target: local"));
    assert!(body.contains("smoke: ok"));

    let write = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "write",
            "notes/result.txt",
            "--content",
            "sandbox data",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        write.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&write.stderr)
    );

    let read = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "read",
            "notes/result.txt",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(read.status.success());
    assert_eq!(String::from_utf8_lossy(&read.stdout), "sandbox data");

    let exec = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "exec",
            "cat notes/result.txt",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(exec.status.success());
    assert_eq!(String::from_utf8_lossy(&exec.stdout), "sandbox data");

    let exec_json = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "exec",
            "printf sandbox-json; printf sandbox-err >&2",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        exec_json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&exec_json.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&exec_json.stderr), "");
    let body: serde_json::Value = serde_json::from_slice(&exec_json.stdout).unwrap();
    assert_eq!(body["target"], "local");
    assert_eq!(
        body["command"],
        "printf sandbox-json; printf sandbox-err >&2"
    );
    assert_eq!(body["success"], true);
    assert_eq!(body["exitCode"], 0);
    assert_eq!(body["stdout"], "sandbox-json");
    assert_eq!(body["stderr"], "sandbox-err");

    let list = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "ls",
            "notes",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("result.txt"));
}

#[test]
fn sandbox_status_json_exposes_agent_readable_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("sandbox-json-status");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "sandbox-json-status",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(scaffold.status.success());

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(setup.status.success());

    let write = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "write",
            "notes/status.json",
            "--content",
            "ready",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(write.status.success());

    let status = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "status",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(body["target"], "local");
    assert_eq!(body["cwd"], ".");
    assert_eq!(body["configured"], true);
    assert_eq!(body["smoke"]["ok"], true);
    assert_eq!(body["capabilities"]["exec"], true);
    assert_eq!(body["capabilities"]["write"], true);
    assert_eq!(body["capabilities"]["cleanup"], true);
    assert_eq!(body["recentLogCount"], 1);
    assert!(body["recentLogs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line.as_str().unwrap().contains("write notes/status.json")));
}

#[test]
fn sandbox_status_json_smokes_remote_endpoint() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("remote-status");
    fs::create_dir_all(&project).unwrap();
    let requests_file = temp.path().join("remote-status-requests.jsonl");
    let endpoint = format!("file://{}", requests_file.display());

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "custom",
            "--endpoint",
            &endpoint,
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let status = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "status",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(body["target"], "custom");
    assert_eq!(body["endpoint"], endpoint);
    assert_eq!(body["smoke"]["ok"], true);
    assert!(body["smoke"]["detail"]
        .as_str()
        .unwrap()
        .contains("remote endpoint smoke ok"));
    assert_eq!(body["capabilities"]["httpEndpoint"], true);
    assert_eq!(body["capabilities"]["remoteConnectorRequired"], false);

    let requests = fs::read_to_string(requests_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["op"], "exec");
    assert_eq!(requests[0]["command"], "pwd");
    assert_eq!(requests[0]["cwd"], ".");
    assert_eq!(requests[1], json!({"op": "readdir", "path": "."}));
}

#[test]
fn sandbox_local_sync_logs_and_cleanup_work_from_cli() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("sandbox-sync");
    let source = temp.path().join("seed");
    fs::create_dir_all(&source).unwrap();
    fs::write(source.join("input.txt"), "seed data").unwrap();

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "sandbox-sync",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(scaffold.status.success());

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(setup.status.success());

    let sync = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "sync",
            source.to_str().unwrap(),
            "workspace-seed",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        sync.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&sync.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("workspace-seed/input.txt")).unwrap(),
        "seed data"
    );

    let remove = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "rm",
            "workspace-seed",
            "--recursive",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        remove.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&remove.stderr)
    );
    assert!(!project.join("workspace-seed").exists());

    let logs = Command::new(agentic_harness_bin())
        .args(["sandbox", "logs", "--workspace", project.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(logs.status.success());
    let logs = String::from_utf8_lossy(&logs.stdout);
    assert!(logs.contains("sync workspace-seed"));
    assert!(logs.contains("rm workspace-seed"));

    let json_logs = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "logs",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        json_logs.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json_logs.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json_logs.stdout).unwrap();
    assert_eq!(
        body["workspace"],
        project.canonicalize().unwrap().display().to_string()
    );
    assert!(body["logPath"]
        .as_str()
        .unwrap()
        .ends_with(".agentic-harness/sandbox.log"));
    assert_eq!(body["count"], 2);
    assert_eq!(body["entries"][0]["index"], 1);
    assert_eq!(body["entries"][0]["message"], "sync workspace-seed");
    assert_eq!(body["entries"][1]["index"], 2);
    assert_eq!(body["entries"][1]["message"], "rm workspace-seed");
}

#[test]
fn sandbox_custom_endpoint_runs_remote_operations_from_cli() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("remote-sandbox");
    fs::create_dir_all(&project).unwrap();
    let requests_file = temp.path().join("remote-requests.jsonl");
    let endpoint = format!("file://{}", requests_file.display());

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "custom",
            "--endpoint",
            &endpoint,
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let status = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "status",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(status.status.success());
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(status.contains("target: custom"));
    assert!(status.contains("endpoint: file://"));
    assert!(status.contains("smoke: ok - remote endpoint smoke ok"));

    let exec = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "exec",
            "pwd",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(exec.status.success());
    assert_eq!(String::from_utf8_lossy(&exec.stdout), "remote:pwd");

    let write = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "write",
            "notes/remote.txt",
            "--content",
            "remote data",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(write.status.success());

    let read = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "read",
            "notes/remote.txt",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(read.status.success());
    assert_eq!(
        String::from_utf8_lossy(&read.stdout),
        "remote-file:notes/remote.txt"
    );

    let list = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "ls",
            ".",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(list.status.success());
    assert!(String::from_utf8_lossy(&list.stdout).contains("remote.txt"));

    let remove = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "rm",
            "notes/remote.txt",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(remove.status.success());

    let requests = fs::read_to_string(requests_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(requests.len(), 7);
    assert_eq!(requests[0]["op"], "exec");
    assert_eq!(requests[0]["cwd"], ".");
    assert_eq!(requests[0]["command"], "pwd");
    assert_eq!(requests[1], json!({"op": "readdir", "path": "."}));
    assert_eq!(
        requests[2],
        json!({"op": "exec", "command": "pwd", "cwd": ".", "env": {}})
    );
    assert_eq!(
        requests[3],
        json!({"op": "write", "path": "notes/remote.txt", "content": "remote data"})
    );
    assert_eq!(
        requests[4],
        json!({"op": "read", "path": "notes/remote.txt"})
    );
    assert_eq!(requests[5], json!({"op": "readdir", "path": "."}));
    assert_eq!(
        requests[6],
        json!({"op": "rm", "path": "notes/remote.txt", "recursive": false})
    );
}

#[test]
fn code_command_runs_the_coding_template_with_one_short_command() {
    let project = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Fix the failing tests",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["prompt"], "Fix the failing tests");
    assert_eq!(body["repo"], ".");
    assert!(body["plan"].as_array().is_some_and(|plan| !plan.is_empty()));
    assert_eq!(body["checks"][0], "cargo test");
}

#[test]
fn start_opens_the_guided_tui_front_door() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("start-front-door-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "start-front-door-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let output = Command::new(agentic_harness_bin())
        .args(["start", "--workspace", project.to_str().unwrap(), "--plain"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(body.contains("Agentic Harness Wizard"));
    assert!(body.contains("Start coding"));
    assert!(body.contains("Create or install template"));
    assert!(body.contains("Set up LLM authoring environment"));
    assert!(body.contains("Set up sandbox"));
    assert!(body.contains(&format!(
        "Workspace: {}",
        project.canonicalize().unwrap().display()
    )));
    assert!(body.contains(&format!(
        "agentic-harness code --workspace {}",
        project.display()
    )));
    assert!(!project.join(".agentic-harness/runs/latest.json").exists());
}

#[test]
fn code_command_without_prompt_uses_actionable_default_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("default-prompt-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "default-prompt-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--no-tests",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["prompt"],
        "Inspect the repository, plan the smallest safe coding step, run the available checks, and summarize the result."
    );
    assert_ne!(body["prompt"], "Inspect the project");
}

#[test]
fn code_command_writes_latest_summary_and_inspect_reads_it() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("latest-summary-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "latest-summary-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Inspect the generated project",
            "--no-tests",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let latest_md = project.join(".agentic-harness/runs/latest.md");
    let latest_json = project.join(".agentic-harness/runs/latest.json");
    assert!(latest_md.exists());
    assert!(latest_json.exists());
    let body = fs::read_to_string(&latest_md).unwrap();
    assert!(body.contains("# Agentic Harness Coding Run"));
    assert!(body.contains("Prompt: Inspect the generated project"));
    assert!(body.contains("not a git repository"));
    assert!(!body.contains("usage: git diff"));

    let inspect = Command::new(agentic_harness_bin())
        .args(["inspect", "--workspace", project.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        inspect.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    assert_eq!(inspect.stdout, fs::read(&latest_md).unwrap());

    let inspect_json = Command::new(agentic_harness_bin())
        .args([
            "inspect",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        inspect_json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&inspect_json.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&inspect_json.stdout).unwrap();
    assert_eq!(json["prompt"], "Inspect the generated project");
    assert_eq!(json["agent"]["status"], "passed");
}

#[test]
fn code_command_writes_mandatory_run_artifact_bundle() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("artifact-bundle-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "artifact-bundle-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--id",
            "artifact-run",
            "--prompt",
            "Inspect the generated project and record artifacts",
            "--no-tests",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let run_dir = project.join(".agentic-harness/runs/artifact-run");
    for name in [
        "run.json",
        "summary.md",
        "events.jsonl",
        "diff.patch",
        "checks.json",
        "agent-instructions.md",
    ] {
        assert!(
            run_dir.join(name).exists(),
            "missing run artifact {}",
            run_dir.join(name).display()
        );
    }

    let run_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(run_dir.join("run.json")).unwrap()).unwrap();
    assert_eq!(run_json["id"], "artifact-run");
    assert_eq!(
        run_json["prompt"],
        "Inspect the generated project and record artifacts"
    );
    assert_eq!(run_json["artifacts"]["summary"], "summary.md");
    assert_eq!(run_json["artifacts"]["events"], "events.jsonl");
    assert_eq!(run_json["artifacts"]["diff"], "diff.patch");
    assert_eq!(run_json["artifacts"]["checks"], "checks.json");
    assert_eq!(
        run_json["artifacts"]["agentInstructions"],
        "agent-instructions.md"
    );

    let checks_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(run_dir.join("checks.json")).unwrap()).unwrap();
    assert_eq!(checks_json.as_array().unwrap().len(), 0);

    let events = fs::read_to_string(run_dir.join("events.jsonl")).unwrap();
    assert!(events
        .lines()
        .any(|line| line.contains(r#""phase":"inspect""#)));
    for line in events.lines() {
        serde_json::from_str::<serde_json::Value>(line).unwrap();
    }

    let instructions = fs::read_to_string(run_dir.join("agent-instructions.md")).unwrap();
    assert!(instructions.contains("# Agentic Harness Agent Instructions"));
    assert!(instructions.contains("Inspect the generated project and record artifacts"));
    assert!(instructions.contains("Approval gates"));
    assert!(instructions
        .contains("Do not commit or open a pull request unless the run explicitly requested it."));
}

#[test]
fn code_command_writes_run_summary_and_executes_checks() {
    let temp = tempfile::tempdir().unwrap();
    let project = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let summary = temp.path().join("coding-summary.md");
    let check_file = temp.path().join("check-ran.txt");
    let check_command = format!("printf checked > {}", check_file.display());

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Inspect the project and report next steps",
            "--test",
            &check_command,
            "--summary",
            summary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[agentic-harness] inspect:"));
    assert!(stderr.contains("[agentic-harness] plan:"));
    assert!(stderr.contains("[agentic-harness] agent: code"));
    assert!(stderr.contains(&format!("[agentic-harness] check: {check_command}")));
    assert!(stderr.contains("[agentic-harness] summary:"));
    assert_eq!(stderr.matches("[agentic-harness] inspect:").count(), 1);
    assert_eq!(stderr.matches("[agentic-harness] plan:").count(), 1);
    assert_eq!(stderr.matches("[agentic-harness] agent: code").count(), 1);
    assert_eq!(
        stderr
            .matches(&format!("[agentic-harness] check: {check_command}"))
            .count(),
        1
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(result["plannedSteps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step.as_str().unwrap().contains("Run checks")));
    assert!(result["inspect"]["workspaceInstructions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "AGENTS.md"
            && entry["content"]
                .as_str()
                .unwrap()
                .contains("native Agentic Harness example agent")));
    assert!(result["inspect"]["workspaceInstructions"]
        .as_array()
        .unwrap()
        .iter()
        .any(|entry| entry["path"] == "CLAUDE.md"
            && entry["content"]
                .as_str()
                .unwrap()
                .contains("Claude Code compatibility")));
    assert!(result["inspect"]["projectFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "Cargo.toml"));
    assert!(result["inspect"]["gitDiffStat"].as_str().is_some());
    assert_eq!(fs::read_to_string(check_file).unwrap(), "checked");
    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("# Agentic Harness Coding Run"));
    assert!(body.contains("Prompt: Inspect the project and report next steps"));
    assert!(body.contains("## Plan"));
    assert!(body.contains("## Project Context"));
    assert!(body.contains("Project files:"));
    assert!(body.contains("Cargo.toml"));
    assert!(body.contains("Git diff stat:"));
    assert!(body.contains("## Sandbox"));
    assert!(body.contains("check mode: `local`"));
    assert!(body.contains("## Workspace Instructions"));
    assert!(body.contains("AGENTS.md"));
    assert!(body.contains("CLAUDE.md"));
    assert!(body.contains("Run checks"));
    assert!(body.contains("## Next Commands"));
    assert!(body.contains("agentic-harness inspect --workspace"));
    assert!(body.contains("agentic-harness dashboard --workspace"));
    assert!(body.contains(&format!("Check: `{check_command}`")));
    assert!(body.contains("status: passed"));
    assert!(body.contains("Agent result"));
}

#[test]
fn code_command_runs_checks_in_configured_remote_sandbox() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("remote-check-agent");
    let requests_file = temp.path().join("remote-check-requests.jsonl");
    let endpoint = format!("file://{}", requests_file.display());

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "remote-check-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    fs::write(project.join("REMOTE_SYNC.md"), "sync me before checks\n").unwrap();

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "custom",
            "--endpoint",
            &endpoint,
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let summary_json = temp.path().join("remote-check-summary.json");
    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Run the configured remote check",
            "--test",
            "printf remote-check",
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(json["checks"][0]["command"], "printf remote-check");
    assert_eq!(json["checks"][0]["status"], "passed");
    assert_eq!(json["checks"][0]["stdout"], "remote:printf remote-check");
    assert_eq!(json["sandbox"]["target"], "custom");
    assert_eq!(json["sandbox"]["cwd"], ".");
    assert_eq!(json["sandbox"]["endpoint"], endpoint);
    assert_eq!(json["sandbox"]["checkMode"], "remote");

    let requests = fs::read_to_string(requests_file)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let sync_index = requests
        .iter()
        .position(|request| {
            request["op"] == "write"
                && request["path"] == "REMOTE_SYNC.md"
                && request["content"] == "sync me before checks\n"
        })
        .unwrap_or_else(|| {
            panic!(
                "coding run did not sync workspace file to remote sandbox; requests: {requests:#?}"
            )
        });
    let exec_index = requests
        .iter()
        .position(|request| {
            request["op"] == "exec"
                && request["command"] == "printf remote-check"
                && request["cwd"] == "."
        })
        .expect("coding run did not execute remote check");
    assert!(
        sync_index < exec_index,
        "workspace sync must happen before remote check execution"
    );
}

#[test]
fn code_command_applies_patch_before_checks_and_summary() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("patched-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "patched-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    let summary = temp.path().join("coding-summary.md");
    let summary_json = temp.path().join("coding-summary.json");
    let patch = temp.path().join("change.patch");
    fs::write(
        &patch,
        r#"diff --git a/WORK.md b/WORK.md
new file mode 100644
index 0000000..5556ed2
--- /dev/null
+++ b/WORK.md
@@ -0,0 +1 @@
+patched
"#,
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Apply the prepared patch",
            "--apply",
            patch.to_str().unwrap(),
            "--test",
            "test -f WORK.md",
            "--summary",
            summary.to_str().unwrap(),
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("WORK.md")).unwrap(),
        "patched\n"
    );
    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("## Applied patches"));
    assert!(body.contains("## Coding Loop"));
    assert!(body.contains("- inspect: completed"));
    assert!(body.contains("- plan: completed"));
    assert!(body.contains("- edit: applied"));
    assert!(body.contains("- test: passed"));
    assert!(body.contains("- summarize: completed"));
    assert!(body.contains("- commit: skipped"));
    assert!(body.contains("- pull-request: skipped"));
    assert!(body.contains("status: applied"));
    assert!(body.contains("WORK.md"));
    assert!(body.contains("## Changed files"));
    assert!(body.contains("## Changed files\n\n- `WORK.md`"));
    assert!(body.contains("Check: `test -f WORK.md`"));

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(json["id"], "code");
    assert_eq!(json["prompt"], "Apply the prepared patch");
    assert_eq!(json["agent"]["status"], "passed");
    assert!(json["plannedSteps"]
        .as_array()
        .unwrap()
        .iter()
        .any(|step| step.as_str().unwrap().contains("Run checks")));
    assert_eq!(json["loop"][0]["phase"], "inspect");
    assert_eq!(json["loop"][0]["status"], "completed");
    assert_eq!(json["loop"][1]["phase"], "plan");
    assert_eq!(json["loop"][1]["status"], "completed");
    assert_eq!(json["loop"][2]["phase"], "edit");
    assert_eq!(json["loop"][2]["status"], "applied");
    assert_eq!(json["loop"][3]["phase"], "test");
    assert_eq!(json["loop"][3]["status"], "passed");
    assert_eq!(json["loop"][4]["phase"], "summarize");
    assert_eq!(json["loop"][4]["status"], "completed");
    assert_eq!(json["loop"][5]["phase"], "commit");
    assert_eq!(json["loop"][5]["status"], "skipped");
    assert_eq!(json["loop"][6]["phase"], "pull-request");
    assert_eq!(json["loop"][6]["status"], "skipped");
    assert!(json["projectFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "Cargo.toml"));
    assert!(json["gitDiffStat"].as_str().is_some());
    assert_eq!(json["sandbox"]["target"], "local");
    assert_eq!(json["sandbox"]["checkMode"], "local");
    assert!(json["changedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "WORK.md"));
    assert_eq!(json["patches"][0]["status"], "applied");
    assert_eq!(json["patches"][0]["files"][0], "WORK.md");
    assert_eq!(json["checks"][0]["command"], "test -f WORK.md");
    assert_eq!(json["checks"][0]["status"], "passed");
    assert!(json["nextCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command
            .as_str()
            .unwrap()
            .starts_with("agentic-harness inspect --workspace")));
    assert!(json["nextCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command
            .as_str()
            .unwrap()
            .starts_with("agentic-harness dashboard --workspace")));
    assert!(json["nextCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command.as_str().unwrap().contains("--prompt")));
}

#[test]
fn code_command_blocks_denied_patch_paths_before_apply() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("denied-path-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "denied-path-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    let original = fs::read_to_string(project.join("src/main.rs")).unwrap();
    let patch = temp.path().join("denied.patch");
    fs::write(
        &patch,
        r#"diff --git a/src/main.rs b/src/main.rs
index 5b3d45e..e9894ec 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
+// denied mutation
 use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
 use serde::Deserialize;
 use serde_json::json;
"#,
    )
    .unwrap();

    let summary_json = temp.path().join("denied-summary.json");
    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Try to change a denied file",
            "--apply",
            patch.to_str().unwrap(),
            "--deny-path",
            "src/",
            "--no-tests",
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(project.join("src/main.rs")).unwrap(),
        original
    );
    let body: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(body["patches"][0]["status"], "blocked");
    assert!(body["patches"][0]["stderr"]
        .as_str()
        .unwrap()
        .contains("denied path src/main.rs"));
    assert_eq!(body["policy"]["denyPaths"][0], "src/");
    assert_eq!(body["loop"][2]["status"], "failed");
}

#[test]
fn code_command_requires_dependency_approval_for_manifest_changes() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("dependency-gate-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "dependency-gate-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    let original = fs::read_to_string(project.join("Cargo.toml")).unwrap();
    let patch = temp.path().join("dependency.patch");
    fs::write(
        &patch,
        r#"diff --git a/Cargo.toml b/Cargo.toml
index 4dc0536..9329bf5 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -9,3 +9,4 @@ publish = false
 agentic-harness = { path = "/tmp/agentic-harness" }
 serde = { version = "1.0", features = ["derive"] }
 serde_json = "1.0"
+regex = "1.0"
"#,
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Try to change dependencies",
            "--apply",
            patch.to_str().unwrap(),
            "--no-tests",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(project.join("Cargo.toml")).unwrap(),
        original
    );
    let run_json: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.join(".agentic-harness/runs/code/run.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(run_json["patches"][0]["status"], "blocked");
    assert!(run_json["patches"][0]["stderr"]
        .as_str()
        .unwrap()
        .contains("dependency change Cargo.toml requires --approve-dependencies"));
    assert_eq!(run_json["policy"]["approveDependencies"], false);
}

#[test]
fn code_command_blocks_checks_above_the_configured_command_risk() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("risk-gate-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "risk-gate-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Run a risky command",
            "--test",
            "rm -rf target",
            "--max-command-risk",
            "medium",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("check command risk high exceeds max-command-risk medium"));
    assert!(!project.join(".agentic-harness/runs/code/run.json").exists());
}

#[test]
fn code_command_applies_agent_generated_patch_before_checks() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("generated-patch-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "generated-patch-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    fs::write(
        project.join("src/main.rs"),
        r##"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde_json::json;

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new().with_workspace(".").agent(AgentDefinition::webhook(
        "code",
        |_ctx: AgentContext| {
            Ok(json!({
                "summary": "Generated a patch for the CLI to apply.",
                "generatedPatch": "diff --git a/GENERATED.md b/GENERATED.md\nnew file mode 100644\nindex 0000000..f61e4d3\n--- /dev/null\n+++ b/GENERATED.md\n@@ -0,0 +1 @@\n+generated by agent\n"
            }))
        },
    )))
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
"##,
    )
    .unwrap();
    let summary = temp.path().join("generated-summary.md");
    let summary_json = temp.path().join("generated-summary.json");

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Create the generated file",
            "--test",
            "test -f GENERATED.md",
            "--summary",
            summary.to_str().unwrap(),
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("GENERATED.md")).unwrap(),
        "generated by agent\n"
    );
    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("## Structured agent result"));
    assert!(body.contains("summary: Generated a patch for the CLI to apply."));
    assert!(body.contains("agent-generated.patch"));
    assert!(body.contains("status: applied"));
    assert!(body.contains("GENERATED.md"));
    assert!(body.contains("Check: `test -f GENERATED.md`"));
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(
        json["agentResult"]["summary"],
        "Generated a patch for the CLI to apply."
    );
    assert!(json["agentResult"]["generatedPatch"]
        .as_str()
        .unwrap()
        .contains("GENERATED.md"));
}

#[test]
fn code_command_can_launch_llm_coder_before_checks() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("llm-coder-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "llm-coder-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(&project)
        .output()
        .unwrap();

    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let capture = temp.path().join("coding-brief.txt");
    let args_capture = temp.path().join("coding-args.txt");
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AGENTIC_HARNESS_CODE_ARGS"
cat > "$AGENTIC_HARNESS_CODE_CAPTURE"
grep -q "Create CODEX_EDIT.md" "$AGENTIC_HARNESS_CODE_CAPTURE" || exit 9
printf 'edited by codex\n' > CODEX_EDIT.md
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_codex).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).unwrap();

    let summary = temp.path().join("llm-coding-summary.md");
    let summary_json = temp.path().join("llm-coding-summary.json");
    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("AGENTIC_HARNESS_CODE_CAPTURE", &capture)
        .env("AGENTIC_HARNESS_CODE_ARGS", &args_capture)
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Create CODEX_EDIT.md",
            "--llm",
            "codex",
            "--test",
            "test -f CODEX_EDIT.md",
            "--summary",
            summary.to_str().unwrap(),
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[agentic-harness] llm: codex"));
    assert_eq!(
        fs::read_to_string(project.join("CODEX_EDIT.md")).unwrap(),
        "edited by codex\n"
    );
    let brief_path = project
        .canonicalize()
        .unwrap()
        .join(".agentic-harness/runs/code/coding-brief.md");
    assert_eq!(
        fs::read_to_string(args_capture).unwrap(),
        "exec\n--sandbox\nworkspace-write\n--skip-git-repo-check\n-\n"
    );
    let brief = fs::read_to_string(capture).unwrap();
    assert!(brief.contains("# Agentic Harness Coding Brief"));
    assert!(brief.contains("Create CODEX_EDIT.md"));
    assert!(brief.contains("Edit files directly in this workspace"));
    assert!(brief.contains("test -f CODEX_EDIT.md"));
    assert_eq!(brief, fs::read_to_string(&brief_path).unwrap());

    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("## LLM coding tool"));
    assert!(body.contains("tool: codex"));
    assert!(body.contains("status: completed"));
    assert!(body.contains("changed files:\n- `CODEX_EDIT.md`"));
    assert!(body.contains("CODEX_EDIT.md"));
    assert!(body.contains("Check: `test -f CODEX_EDIT.md`"));

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(json["llm"]["tool"], "codex");
    assert_eq!(json["llm"]["status"], "completed");
    assert!(json["llm"]["brief"]
        .as_str()
        .unwrap()
        .ends_with(".agentic-harness/runs/code/coding-brief.md"));
    assert_eq!(json["llm"]["changedFiles"][0], "CODEX_EDIT.md");
    assert!(json["changedFiles"]
        .as_array()
        .unwrap()
        .iter()
        .any(|file| file == "CODEX_EDIT.md"));
}

#[test]
fn code_command_records_llm_file_changes_without_git() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("non-git-llm-coder-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "non-git-llm-coder-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();

    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_codex = fake_bin.join("codex");
    fs::write(
        &fake_codex,
        r#"#!/bin/sh
cat >/dev/null
printf 'created outside git\n' > NON_GIT_EDIT.md
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_codex).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_codex, permissions).unwrap();

    let summary = temp.path().join("non-git-llm-summary.md");
    let summary_json = temp.path().join("non-git-llm-summary.json");
    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Create NON_GIT_EDIT.md",
            "--llm",
            "codex",
            "--test",
            "test -f NON_GIT_EDIT.md",
            "--summary",
            summary.to_str().unwrap(),
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(project.join("NON_GIT_EDIT.md")).unwrap(),
        "created outside git\n"
    );

    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("Git status after:"));
    assert!(body.contains("not a git repository"));
    assert!(body.contains("## Changed files\n\n- `NON_GIT_EDIT.md`"));
    assert!(body.contains("- edit: completed - 0 patches, 1 changed file, LLM completed"));
    assert!(body.contains("- summarize: completed - 1 changed file recorded"));

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(json["llm"]["tool"], "codex");
    assert_eq!(json["llm"]["status"], "completed");
    assert_eq!(json["changedFiles"][0], "NON_GIT_EDIT.md");
    assert_eq!(
        json["loop"][2]["detail"],
        "0 patches, 1 changed file, LLM completed"
    );
    assert_eq!(json["loop"][4]["detail"], "1 changed file recorded");
}

#[test]
fn code_command_auto_llm_uses_first_installed_coder_before_checks() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("auto-llm-coder-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "auto-llm-coder-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(&project)
        .output()
        .unwrap();

    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let capture = temp.path().join("auto-coding-brief.txt");
    let args_capture = temp.path().join("auto-coding-args.txt");
    let fake_claude = fake_bin.join("claude");
    fs::write(
        &fake_claude,
        r#"#!/bin/sh
printf '%s\n' "$1" "$2" "$3" > "$AGENTIC_HARNESS_CODE_ARGS"
printf '%s' "$4" > "$AGENTIC_HARNESS_CODE_CAPTURE"
grep -q "Create AUTO_EDIT.md" "$AGENTIC_HARNESS_CODE_CAPTURE" || exit 9
printf 'edited by claude\n' > AUTO_EDIT.md
"#,
    )
    .unwrap();
    let fake_codex = fake_bin.join("codex");
    fs::write(&fake_codex, "#!/bin/sh\nexit 88\n").unwrap();
    for fake in [&fake_claude, &fake_codex] {
        let mut permissions = fs::metadata(fake).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(fake, permissions).unwrap();
    }

    let summary_json = temp.path().join("auto-llm-coding-summary.json");
    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .env("AGENTIC_HARNESS_CODE_CAPTURE", &capture)
        .env("AGENTIC_HARNESS_CODE_ARGS", &args_capture)
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Create AUTO_EDIT.md",
            "--llm",
            "auto",
            "--test",
            "test -f AUTO_EDIT.md",
            "--summary-json",
            summary_json.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[agentic-harness] llm: claude-code"));
    assert_eq!(
        fs::read_to_string(project.join("AUTO_EDIT.md")).unwrap(),
        "edited by claude\n"
    );
    let brief_path = project
        .canonicalize()
        .unwrap()
        .join(".agentic-harness/runs/code/coding-brief.md");
    assert_eq!(
        fs::read_to_string(args_capture).unwrap(),
        "-p\n--permission-mode\nacceptEdits\n"
    );
    assert_eq!(
        fs::read_to_string(capture).unwrap(),
        fs::read_to_string(&brief_path).unwrap()
    );

    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(summary_json).unwrap()).unwrap();
    assert_eq!(json["llm"]["environment"], "claude-code");
    assert_eq!(json["llm"]["tool"], "claude");
    assert_eq!(json["llm"]["status"], "completed");
}

#[test]
fn code_command_retries_failed_checks_with_repair_patch() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("repair-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "repair-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    fs::write(
        project.join("src/main.rs"),
        r##"use agentic_harness::{run_cli, AgentApp, AgentContext, AgentDefinition, AgenticHarnessError};
use serde_json::{json, Value};

fn app() -> Result<AgentApp, AgenticHarnessError> {
    Ok(AgentApp::new().with_workspace(".").agent(AgentDefinition::webhook(
        "code",
        |ctx: AgentContext| {
            let payload: Value = ctx.payload()?;
            let repairing = payload
                .get("repairAttempt")
                .and_then(Value::as_u64)
                .unwrap_or_default()
                > 0;
            if repairing {
                Ok(json!({
                    "summary": "Repair generated a patch after seeing failed checks.",
                    "generatedPatch": "diff --git a/REPAIRED.md b/REPAIRED.md\nnew file mode 100644\nindex 0000000..21bb160\n--- /dev/null\n+++ b/REPAIRED.md\n@@ -0,0 +1 @@\n+repaired by agent\n"
                }))
            } else {
                Ok(json!({
                    "summary": "Initial pass intentionally waits for check feedback.",
                    "sawFailedChecks": payload.get("failedChecks").is_some()
                }))
            }
        },
    )))
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
"##,
    )
    .unwrap();
    let summary = temp.path().join("repair-summary.md");

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Create the repaired file after tests fail",
            "--test",
            "test -f REPAIRED.md",
            "--summary",
            summary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[agentic-harness] repair: 1 failed check"));
    assert_eq!(
        fs::read_to_string(project.join("REPAIRED.md")).unwrap(),
        "repaired by agent\n"
    );
    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("agent-generated.patch"));
    assert!(body.contains("status: applied"));
    assert!(body.contains("Check: `test -f REPAIRED.md`"));
    assert!(body.contains("status: passed"));
}

#[test]
fn code_command_can_commit_successful_coding_run() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("commit-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "commit-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "agentic-harness@example.test"])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Agentic Harness"])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "baseline"])
        .current_dir(&project)
        .output()
        .unwrap();

    let summary = temp.path().join("coding-summary.md");
    let patch = temp.path().join("commit.patch");
    fs::write(
        &patch,
        r#"diff --git a/COMMIT.md b/COMMIT.md
new file mode 100644
index 0000000..926dfc9
--- /dev/null
+++ b/COMMIT.md
@@ -0,0 +1 @@
+committed
"#,
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Apply and commit the prepared patch",
            "--apply",
            patch.to_str().unwrap(),
            "--test",
            "test -f COMMIT.md",
            "--commit",
            "Apply prepared patch",
            "--summary",
            summary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let log = Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(&project)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&log.stdout).trim(),
        "Apply prepared patch"
    );
    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("## Commit"));
    assert!(body.contains("status: committed"));
    assert!(body.contains("Apply prepared patch"));
}

#[test]
fn code_command_can_create_pull_request_after_successful_run() {
    let temp = tempfile::tempdir().unwrap();
    let source = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let project = temp.path().join("pr-agent");
    copy_test_workspace(&source, &project);
    let crate_path = std::env::current_dir()
        .unwrap()
        .join("../../crates/agentic-harness")
        .canonicalize()
        .unwrap();
    fs::write(
        project.join("Cargo.toml"),
        format!(
            r#"[package]
name = "pr-agent"
version = "0.1.0"
edition = "2021"
publish = false

[dependencies]
agentic-harness = {{ path = "{}" }}
serde = {{ version = "1.0", features = ["derive"] }}
serde_json = "1.0"
"#,
            crate_path.display()
        ),
    )
    .unwrap();
    fs::copy(
        std::env::current_dir().unwrap().join("../../Cargo.lock"),
        project.join("Cargo.lock"),
    )
    .unwrap();
    Command::new("git")
        .args(["init"])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.email", "agentic-harness@example.test"])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["config", "user.name", "Agentic Harness"])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(&project)
        .output()
        .unwrap();
    Command::new("git")
        .args(["commit", "-m", "baseline"])
        .current_dir(&project)
        .output()
        .unwrap();

    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let gh_args = temp.path().join("gh-args.txt");
    let fake_gh = fake_bin.join("gh");
    fs::write(
        &fake_gh,
        r#"#!/bin/sh
printf '%s\n' "$@" > "$AGENTIC_HARNESS_GH_ARGS"
echo https://github.com/example/repo/pull/1
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_gh).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_gh, permissions).unwrap();
    let path = format!(
        "{}:{}",
        fake_bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let summary = temp.path().join("coding-summary.md");
    let patch = temp.path().join("pr.patch");
    fs::write(
        &patch,
        r#"diff --git a/PR.md b/PR.md
new file mode 100644
index 0000000..d8bf8a4
--- /dev/null
+++ b/PR.md
@@ -0,0 +1 @@
+pull request
"#,
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .env("PATH", path)
        .env("AGENTIC_HARNESS_GH_ARGS", &gh_args)
        .args([
            "code",
            "--workspace",
            project.to_str().unwrap(),
            "--prompt",
            "Apply and open a pull request",
            "--apply",
            patch.to_str().unwrap(),
            "--test",
            "test -f PR.md",
            "--commit",
            "Apply PR patch",
            "--pr",
            "--summary",
            summary.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let args = fs::read_to_string(gh_args).unwrap();
    assert!(args.contains("pr\ncreate\n--fill"));
    let body = fs::read_to_string(summary).unwrap();
    assert!(body.contains("## Pull request"));
    assert!(body.contains("status: created"));
    assert!(body.contains("https://github.com/example/repo/pull/1"));
}

#[test]
fn doctor_reports_ready_generated_project() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("coding-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "coding-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();

    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let output = Command::new(agentic_harness_bin())
        .args([
            "doctor",
            "--workspace",
            project.to_str().unwrap(),
            "--plain",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(body.contains("Agentic Harness Doctor"));
    assert!(body.contains("Cargo.toml: ok"));
    assert!(body.contains("src/main.rs: ok"));
    assert!(body.contains("AGENTS.md: ok"));
    assert!(body.contains("roles: ok"));
    assert!(body.contains("skills: ok"));
    assert!(body.contains("git repo:"));
    assert!(body.contains("worktree:"));
    assert!(body.contains("tests:"));
    assert!(body.contains("permissions: ok"));
    assert!(body.contains("recovery: ok"));
    assert!(body.contains("ready: workspace can run with agentic-harness"));
}

#[test]
fn doctor_reports_missing_workspace_with_next_step() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("empty-agent");
    fs::create_dir_all(&project).unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "doctor",
            "--workspace",
            project.to_str().unwrap(),
            "--plain",
        ])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(body.contains("Agentic Harness Doctor"));
    assert!(body.contains("Cargo.toml: missing"));
    assert!(body.contains("src/main.rs: missing"));
    assert!(body.contains("not ready: missing required files"));
    assert!(body.contains("agentic-harness new ./my-agent --name my-agent --template hello"));
}

#[test]
fn doctor_json_reports_required_and_optional_checks() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("doctor-json-agent");
    fs::create_dir_all(&project).unwrap();

    let output = Command::new(agentic_harness_bin())
        .args(["doctor", "--workspace", project.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        body["workspace"],
        project.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(body["requiredReady"], false);
    assert_eq!(body["optionalWarnings"], true);
    assert_eq!(
        body["nextCommand"],
        "agentic-harness new ./my-agent --name my-agent --template hello"
    );
    assert!(body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["label"] == "Cargo.toml"
            && check["ok"] == false
            && check["required"] == true
            && check["fix"]
                .as_str()
                .unwrap()
                .contains("agentic-harness new")));
    assert!(body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["label"] == "llm authoring"
            && check["ok"] == false
            && check["required"] == false));
    assert!(body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["label"] == "llm tools"
            && check["required"] == false
            && check["detail"].as_str().unwrap().contains("Claude Code")
            && check["detail"].as_str().unwrap().contains("Codex")
            && check["detail"].as_str().unwrap().contains("Cursor")
            && check["detail"].as_str().unwrap().contains("Wind Server")));
}

#[test]
fn run_invokes_a_native_rust_agent_workspace() {
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "run",
            "hello",
            "--workspace",
            workspace.to_str().unwrap(),
            "--id",
            "test-1",
            "--payload",
            r#"{"name":"Ada"}"#,
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        result,
        json!({
            "id": "test-1",
            "message": "Hello, Ada!",
            "role": "Warm concise greeter"
        })
    );
}

#[test]
fn agentic_harness_binary_invokes_the_primary_native_cli() {
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "run",
            "hello",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "native",
            "--id",
            "agentic-harness-primary",
            "--payload",
            r#"{"name":"Harness"}"#,
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["id"], "agentic-harness-primary");
    assert_eq!(result["message"], "Hello, Harness!");
}

#[test]
fn wizard_plain_renders_runtime_dashboard_and_next_commands() {
    let output = Command::new(agentic_harness_bin())
        .args(["wizard", "--plain"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(body.contains("Agentic Harness Wizard"));
    assert!(body.contains("Status panel"));
    assert!(body.contains("Workspace:"));
    assert!(body.contains("Readiness:"));
    assert!(body.contains("Sandbox:"));
    assert!(body.contains("Templates:"));
    assert!(body.contains("Recent logs:"));
    assert!(body.contains("Pick a workflow number first, then pick an option number"));
    assert!(body.contains(
        "Selection screens show status panels, template previews, sandbox logs, and next commands"
    ));
    assert!(body.contains("Start coding"));
    assert!(body.contains("Create or install template"));
    assert!(body.contains("Set up LLM authoring environment"));
    assert!(body.contains("Set up sandbox"));
    assert!(body.contains("Check setup"));
    assert!(body.contains("Run an agent"));
    assert!(body.contains("agentic-harness code"));
    assert!(body.contains("agentic-harness code --workspace . --llm auto"));
    assert!(body.contains("agentic-harness tpl create"));
    assert!(body.contains("agentic-harness template author"));
    assert!(body.contains("agentic-harness template export"));
    assert!(body.contains("agentic-harness tpl use"));
    assert!(body.contains("agentic-harness templates ls"));
    assert!(body.contains("agentic-harness setup llm"));
    assert!(body.contains("agentic-harness setup sandbox --target local"));
    assert!(body.contains("agentic-harness setup sandbox --target e2b --print"));
    assert!(body.contains("agentic-harness sandbox sync"));
    assert!(body.contains("agentic-harness sandbox logs"));
    assert!(body.contains("agentic-harness sandbox rm"));
    assert!(body.contains("agentic-harness inspect --workspace ."));
    assert!(body.contains("agentic-harness dashboard --workspace ."));
    assert!(body.contains("agentic-harness doctor --workspace ."));
    assert!(body.contains("Run from your checkout"));
    assert!(!body.contains("<agent>"));
    assert!(!body.contains("<json>"));
    assert!(!body.contains("<projectUrl>"));
    assert!(!body.contains("<accessApiKey>"));
    assert!(!body.contains("Host with"));
    assert!(!body.contains("InsForge"));
    assert!(!body.contains("Cloudflare"));
}

#[test]
fn guide_prints_start_here_path_for_humans_and_agents() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = temp.path().join("guided-agent");
    fs::create_dir_all(&workspace).unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "guide",
            "--workspace",
            workspace.to_str().unwrap(),
            "--env",
            "codex",
            "--template",
            "bugfix-agent",
            "--prompt",
            "Create a bugfix coding template",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(body.contains("Agentic Harness Start Guide"));
    assert!(body.contains("1. Install CLI"));
    assert!(body.contains("2. Set up LLM authoring"));
    assert!(body.contains(&format!(
        "agentic-harness setup llm --workspace {} --env codex",
        workspace.display()
    )));
    assert!(body.contains("3. Create reusable template"));
    assert!(body.contains(&format!(
        "agentic-harness template author bugfix-agent --workspace {} --env codex --prompt \"Create a bugfix coding template\" --open",
        workspace.display()
    )));
    assert!(body.contains("4. Start coding"));
    assert!(body.contains(&format!(
        "agentic-harness code --workspace {} --prompt \"Describe the software change\" --llm codex",
        workspace.display()
    )));
    assert!(body.contains("5. Inspect result"));
    assert!(body.contains(&format!(
        "agentic-harness inspect --workspace {}",
        workspace.display()
    )));
    assert!(!body.contains("<workspace>"));

    let json = Command::new(agentic_harness_bin())
        .args([
            "quickstart",
            "--workspace",
            workspace.to_str().unwrap(),
            "--env",
            "codex",
            "--template",
            "bugfix-agent",
            "--prompt",
            "Create a bugfix coding template",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        body["workspace"],
        workspace.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(body["steps"].as_array().unwrap().len(), 5);
    assert_eq!(body["steps"][1]["id"], "setup-llm");
    assert_eq!(
        body["steps"][1]["command"],
        format!(
            "agentic-harness setup llm --workspace {} --env codex",
            workspace.display()
        )
    );
    assert_eq!(body["steps"][2]["id"], "author-template");
    assert_eq!(body["steps"][3]["id"], "start-coding");
    assert_eq!(body["steps"][4]["id"], "inspect-result");
    assert_eq!(
        body["steps"][4]["command"],
        format!(
            "agentic-harness inspect --workspace {}",
            workspace.display()
        )
    );
}

#[test]
fn bare_command_prints_plain_tui_map_in_non_tty() {
    let output = Command::new(agentic_harness_bin()).output().unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8_lossy(&output.stdout);
    assert!(body.contains("Agentic Harness Wizard"));
    assert!(body.contains("Start coding"));
    assert!(body.contains("Set up sandbox"));
}

#[test]
fn tui_and_ui_aliases_open_the_wizard_dashboard() {
    for alias in ["tui", "ui"] {
        let output = Command::new(agentic_harness_bin())
            .args([alias, "--plain"])
            .output()
            .unwrap();

        assert!(
            output.status.success(),
            "{alias} stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let body = String::from_utf8_lossy(&output.stdout);
        assert!(body.contains("Agentic Harness Wizard"));
        assert!(body.contains("Start coding"));
        assert!(body.contains("Set up sandbox"));
    }
}

#[test]
fn tui_plain_accepts_workspace_and_renders_that_workspace_status() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("tui-workspace-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "tui-workspace-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let setup_sandbox = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(setup_sandbox.status.success());

    let write = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "write",
            "notes/tui.txt",
            "--content",
            "workspace tui data",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(write.status.success());

    let output = Command::new(agentic_harness_bin())
        .args(["tui", "--workspace", project.to_str().unwrap(), "--plain"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body = String::from_utf8_lossy(&output.stdout);
    let canonical_project = project.canonicalize().unwrap();
    assert!(body.contains("Agentic Harness Wizard"));
    assert!(body.contains(&format!("Workspace: {}", canonical_project.display())));
    assert!(body.contains("Recent logs: 1 recent entry"));
    assert!(body.contains(&format!(
        "Start coding here: agentic-harness code --workspace {}",
        project.display()
    )));
    assert!(body.contains(&format!(
        "agentic-harness code --workspace {}",
        project.display()
    )));
    assert!(body.contains(&format!(
        "Inspect latest result: agentic-harness inspect --workspace {}",
        project.display()
    )));
    assert!(body.contains(&format!(
        "agentic-harness dashboard --workspace {}",
        project.display()
    )));
    assert!(!body.contains("Workspace: ."));
}

#[test]
fn dashboard_plain_shows_workspace_templates_sandbox_logs_and_next_steps() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("dashboard-agent");
    let template = temp.path().join("dashboard-template");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "dashboard-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(scaffold.status.success());

    let setup_llm = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(setup_llm.status.success());

    let setup_sandbox = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(setup_sandbox.status.success());

    let author_brief = Command::new(agentic_harness_bin())
        .args([
            "template",
            "author",
            "dashboard-review",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create a dashboard review coding template",
        ])
        .output()
        .unwrap();
    assert!(
        author_brief.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&author_brief.stderr)
    );

    let init_template = Command::new(agentic_harness_bin())
        .args([
            "template",
            "init",
            template.to_str().unwrap(),
            "--name",
            "dashboard",
            "--agent",
            "dashboard",
            "--version",
            "0.3.0",
        ])
        .output()
        .unwrap();
    assert!(init_template.status.success());

    let install_template = Command::new(agentic_harness_bin())
        .args([
            "template",
            "install",
            template.to_str().unwrap(),
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(install_template.status.success());

    let write = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "write",
            "notes/status.txt",
            "--content",
            "dashboard data",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(write.status.success());
    write_latest_coding_run(&project);

    let dashboard = Command::new(agentic_harness_bin())
        .args([
            "dashboard",
            "--workspace",
            project.to_str().unwrap(),
            "--plain",
        ])
        .output()
        .unwrap();
    assert!(
        dashboard.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dashboard.stderr)
    );
    let body = String::from_utf8_lossy(&dashboard.stdout);
    assert!(body.contains("Agentic Harness Dashboard"));
    assert!(body.contains("Workspace"));
    assert!(body.contains("Required checks: ready"));
    assert!(body.contains("Templates"));
    assert!(body.contains("dashboard (agent dashboard, version 0.3.0"));
    assert!(body.contains("Template briefs"));
    assert!(body.contains("dashboard-review.md"));
    assert!(body.contains("Create a dashboard review coding template"));
    assert!(body.contains("Latest coding run"));
    assert!(body.contains("inspect: completed"));
    assert!(body.contains("edit: applied"));
    assert!(body.contains("test: passed"));
    assert!(body.contains("changed files: src/main.rs"));
    assert!(body.contains("Sandbox"));
    assert!(body.contains("target: local"));
    assert!(body.contains("Recent sandbox logs"));
    assert!(body.contains("write notes/status.txt"));
    assert!(body.contains("Next commands"));
    assert!(body.contains("agentic-harness start --workspace"));
    assert!(!body.contains("\x1b["));
}

#[test]
fn dashboard_json_exposes_agent_readable_status() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("dashboard-json-agent");
    let template = temp.path().join("dashboard-json-template");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "dashboard-json-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(scaffold.status.success());

    let setup_llm = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "llm",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
        ])
        .output()
        .unwrap();
    assert!(setup_llm.status.success());

    let setup_sandbox = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "local",
        ])
        .output()
        .unwrap();
    assert!(setup_sandbox.status.success());

    let author_brief = Command::new(agentic_harness_bin())
        .args([
            "template",
            "brief",
            "jsondash-review",
            "--workspace",
            project.to_str().unwrap(),
            "--env",
            "codex",
            "--prompt",
            "Create json dashboard review template",
        ])
        .output()
        .unwrap();
    assert!(
        author_brief.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&author_brief.stderr)
    );

    let init_template = Command::new(agentic_harness_bin())
        .args([
            "template",
            "create",
            template.to_str().unwrap(),
            "--name",
            "jsondash",
            "--agent",
            "jsondash",
            "--version",
            "0.4.0",
        ])
        .output()
        .unwrap();
    assert!(init_template.status.success());

    let install_template = Command::new(agentic_harness_bin())
        .args([
            "tpl",
            "add",
            template.to_str().unwrap(),
            "--workspace",
            project.to_str().unwrap(),
            "--scope",
            "user",
        ])
        .output()
        .unwrap();
    assert!(install_template.status.success());

    let write = Command::new(agentic_harness_bin())
        .args([
            "sandbox",
            "write",
            "notes/status.txt",
            "--content",
            "json dashboard data",
            "--workspace",
            project.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(write.status.success());
    write_latest_coding_run(&project);

    let dashboard = Command::new(agentic_harness_bin())
        .args([
            "dashboard",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        dashboard.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dashboard.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&dashboard.stdout).unwrap();
    assert_eq!(body["requiredReady"], true);
    assert_eq!(body["optionalWarnings"], true);
    assert!(body["workspace"]
        .as_str()
        .unwrap()
        .ends_with("dashboard-json-agent"));
    assert!(body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["label"] == "workspace" && check["ok"] == true));
    assert!(body["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["label"] == "git repo"
            && check["ok"] == false
            && check["detail"]
                .as_str()
                .unwrap()
                .contains("commit/PR gates need git")));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "jsondash"
            && template["source"] == "user"
            && template["version"] == "0.4.0"));
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "coding" && template["source"] == "built-in"));
    assert!(body["templateBriefs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|brief| brief["name"] == "jsondash-review.md"
            && brief["request"] == "Create json dashboard review template"));
    assert_eq!(body["latestCodingRun"]["loop"][0]["phase"], "inspect");
    assert_eq!(body["latestCodingRun"]["loop"][0]["status"], "completed");
    assert_eq!(body["latestCodingRun"]["loop"][2]["phase"], "edit");
    assert_eq!(body["latestCodingRun"]["loop"][2]["status"], "applied");
    assert_eq!(body["latestCodingRun"]["loop"][3]["phase"], "test");
    assert_eq!(body["latestCodingRun"]["loop"][3]["status"], "passed");
    assert_eq!(body["latestCodingRun"]["changedFiles"][0], "src/main.rs");
    assert_eq!(body["sandbox"]["target"], "local");
    assert_eq!(body["sandbox"]["smoke"]["ok"], true);
    assert!(body["recentSandboxLogs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line.as_str().unwrap().contains("write notes/status.txt")));
    assert!(body["nextCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command
            .as_str()
            .unwrap()
            .starts_with("agentic-harness start --workspace")));
}

#[test]
fn dashboard_json_uses_configured_remote_sandbox_smoke() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("dashboard-remote-agent");
    let requests_file = temp.path().join("dashboard-remote-requests.jsonl");
    let endpoint = format!("file://{}", requests_file.display());

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "dashboard-remote-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let setup = Command::new(agentic_harness_bin())
        .args([
            "setup",
            "sandbox",
            "--workspace",
            project.to_str().unwrap(),
            "--target",
            "custom",
            "--endpoint",
            &endpoint,
        ])
        .output()
        .unwrap();
    assert!(
        setup.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&setup.stderr)
    );

    let dashboard = Command::new(agentic_harness_bin())
        .args([
            "dashboard",
            "--workspace",
            project.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        dashboard.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&dashboard.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&dashboard.stdout).unwrap();
    assert_eq!(body["sandbox"]["target"], "custom");
    assert_eq!(body["sandbox"]["endpoint"], endpoint);
    assert_eq!(body["sandbox"]["smoke"]["ok"], true);
    assert!(body["sandbox"]["smoke"]["detail"]
        .as_str()
        .unwrap()
        .starts_with("remote endpoint smoke ok"));
}

#[test]
fn short_status_and_check_aliases_cover_dashboard_and_doctor() {
    let project = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let status = Command::new(agentic_harness_bin())
        .args(["status", "--workspace", project.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(body["requiredReady"], true);
    assert!(body["templates"]
        .as_array()
        .unwrap()
        .iter()
        .any(|template| template["name"] == "coding"));

    let check = Command::new(agentic_harness_bin())
        .args(["check", "--workspace", project.to_str().unwrap(), "--plain"])
        .output()
        .unwrap();
    assert!(
        check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&check.stderr)
    );
    let body = String::from_utf8_lossy(&check.stdout);
    assert!(body.contains("Agentic Harness Doctor"));
    assert!(body.contains("ready: workspace can run with agentic-harness"));
}

#[test]
fn wizard_refuses_non_tty_without_plain_flag() {
    let output = Command::new(agentic_harness_bin())
        .arg("wizard")
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("interactive wizard"));
    assert!(stderr.contains("agentic-harness wizard --plain"));
}

#[test]
fn repository_source_and_docs_do_not_emit_the_previous_product_name() {
    let repo_root = std::env::current_dir()
        .unwrap()
        .join("../..")
        .canonicalize()
        .unwrap();
    let banned_lower = ["fl", "ue"].concat();
    let banned_title = ["Fl", "ue"].concat();
    let banned_upper = ["FL", "UE"].concat();
    let mut offenders = Vec::new();

    collect_previous_name_offenders(
        &repo_root,
        &repo_root,
        &[&banned_lower, &banned_title, &banned_upper],
        &mut offenders,
    );

    assert!(
        offenders.is_empty(),
        "previous product name still appears in:\n{}",
        offenders.join("\n")
    );
}

#[test]
fn sdk_core_native_imports_are_feature_gated() {
    let repo_root = std::env::current_dir()
        .unwrap()
        .join("../..")
        .canonicalize()
        .unwrap();
    let sdk = fs::read_to_string(repo_root.join("crates/agentic-harness/src/lib.rs")).unwrap();

    assert_native_import_is_gated(&sdk, "use std::fs;");
    assert_native_import_is_gated(&sdk, "use std::net::");
    assert_native_import_is_gated(&sdk, "use std::process::");
}

fn assert_native_import_is_gated(source: &str, needle: &str) {
    let lines = source.lines().collect::<Vec<_>>();
    for (index, line) in lines.iter().enumerate() {
        if line.contains(needle) {
            assert_eq!(
                index
                    .checked_sub(1)
                    .and_then(|previous| lines.get(previous)),
                Some(&"#[cfg(feature = \"native\")]"),
                "native import {needle:?} must be gated with #[cfg(feature = \"native\")]"
            );
        }
    }
}

#[test]
fn manifest_prints_agents_from_a_native_workspace() {
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args(["manifest", "--workspace", workspace.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        manifest,
        json!({
            "agents": [
                { "name": "assistant", "triggers": { "webhook": true } },
                { "name": "code", "triggers": { "webhook": true } },
                { "name": "env", "triggers": { "webhook": true } },
                { "name": "hello", "triggers": { "webhook": true } },
                { "name": "shell", "triggers": { "webhook": true } },
                { "name": "triage", "triggers": { "webhook": false } }
            ]
        })
    );
}

#[test]
fn manifest_can_print_cloudflare_worker_metadata_from_a_native_workspace() {
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "manifest",
            "--workspace",
            workspace.to_str().unwrap(),
            "--cloudflare",
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        manifest["webhookAgentNames"],
        json!(["assistant", "code", "env", "hello", "shell"])
    );
    assert_eq!(
        manifest["durableObjects"],
        json!([
            { "agentName": "assistant", "bindingName": "Assistant", "className": "Assistant" },
            { "agentName": "code", "bindingName": "Code", "className": "Code" },
            { "agentName": "env", "bindingName": "Env", "className": "Env" },
            { "agentName": "hello", "bindingName": "Hello", "className": "Hello" },
            { "agentName": "shell", "bindingName": "Shell", "className": "Shell" }
        ])
    );
}

#[test]
fn run_accepts_relative_workspace_paths() {
    let repo_root = std::env::current_dir()
        .unwrap()
        .join("../..")
        .canonicalize()
        .unwrap();
    let output = Command::new(agentic_harness_bin())
        .current_dir(repo_root)
        .args([
            "run",
            "hello",
            "--workspace",
            "examples/hello-world",
            "--id",
            "relative-1",
            "--payload",
            r#"{"name":"Grace"}"#,
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["id"], "relative-1");
    assert_eq!(result["message"], "Hello, Grace!");
}

#[test]
fn run_loads_env_files_for_native_workspaces() {
    let temp = tempfile::tempdir().unwrap();
    let env_path = temp.path().join(".env");
    fs::write(&env_path, "AGENTIC_HARNESS_NATIVE_TEST=from-env-file\n").unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args([
            "run",
            "env",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "native",
            "--id",
            "env-1",
            "--env",
            env_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["value"], "from-env-file");
}

#[test]
fn node_target_builds_node_host_package_for_native_binary() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();

    let run = Command::new(agentic_harness_bin())
        .args([
            "run",
            "hello",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "node",
            "--id",
            "node-alias",
            "--payload",
            r#"{"name":"Node"}"#,
        ])
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&run.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&run.stdout).unwrap();
    assert_eq!(result["message"], "Hello, Node!");

    let output_dir = temp.path().join("node-out");
    let build = Command::new(agentic_harness_bin())
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "node",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    assert!(output_dir.join("dist/agentic-harness-agent").exists());
    assert!(output_dir.join("dist/server.mjs").exists());
    assert!(output_dir.join("dist/package.json").exists());
    let launcher = fs::read_to_string(output_dir.join("dist/server.mjs")).unwrap();
    let package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output_dir.join("dist/package.json")).unwrap())
            .unwrap();
    assert!(launcher.contains("agentic-harness-agent"));
    assert!(launcher.contains("--agentic-harness-serve"));
    assert_eq!(package["scripts"]["start"], "node server.mjs");
    assert!(String::from_utf8_lossy(&build.stderr).contains("Target: node host package"));
}

#[test]
fn cloudflare_run_reports_native_runtime_incompatibility() {
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let run = Command::new(agentic_harness_bin())
        .args([
            "run",
            "hello",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "cloudflare",
            "--id",
            "cf",
        ])
        .output()
        .unwrap();

    assert!(!run.status.success());
    let run_stderr = String::from_utf8_lossy(&run.stderr);
    assert!(run_stderr.contains("Cloudflare Workers target is not available"));
    assert!(run_stderr.contains("native Rust server"));
    assert!(run_stderr.contains("non-proxy Worker-compatible runtime"));
}

#[test]
fn cloudflare_build_generates_non_proxy_worker_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let output_dir = temp.path().join("out");
    let build = Command::new(agentic_harness_bin())
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "cloudflare",
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );

    let dist = output_dir.join("dist");
    let entry = fs::read_to_string(dist.join("_entry.js")).unwrap();
    let runtime = fs::read_to_string(dist.join("agentic_harness_worker.js")).unwrap();
    let app_adapter = fs::read_to_string(dist.join("agentic_harness_app.js")).unwrap();
    let app_contract = fs::read_to_string(dist.join("agentic_harness_app.d.ts")).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dist.join("cloudflare-manifest.json")).unwrap())
            .unwrap();
    let wrangler: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dist.join("wrangler.jsonc")).unwrap()).unwrap();

    assert!(!dist.join("agentic-harness-agent").exists());
    assert!(entry.contains("export class Hello"));
    assert!(entry.contains("DURABLE_OBJECT_BINDINGS"));
    assert!(!entry.contains("127.0.0.1"));
    assert!(!entry.contains("localhost"));
    assert!(runtime.contains("TransformStream"));
    assert!(runtime.contains("agentic_harness_sessions"));
    assert!(runtime.contains("invokeAgenticHarnessAgent"));
    assert!(runtime.contains("text/event-stream"));
    assert!(!runtime.contains("runtime_not_linked"));
    assert!(app_adapter.contains("handler_not_linked"));
    assert!(app_contract.contains("AgenticHarnessWorkerContext"));
    assert!(app_contract.contains("sessionStore: AgenticHarnessSessionStore"));
    assert!(app_contract.contains("emit(event: AgenticHarnessWorkerEvent): void"));
    assert!(app_contract.contains("invokeAgenticHarnessAgent"));
    assert_eq!(
        manifest["webhookAgentNames"],
        json!(["assistant", "code", "env", "hello", "shell"])
    );
    assert_eq!(wrangler["main"], "_entry.js");
    assert_eq!(wrangler["compatibility_date"], "2026-05-08");
    assert_eq!(
        wrangler["durable_objects"]["bindings"][3],
        json!({ "name": "Hello", "class_name": "Hello" })
    );
    assert_eq!(
        wrangler["migrations"][3],
        json!({ "tag": "agentic-harness-Hello", "new_sqlite_classes": ["Hello"] })
    );
}

#[test]
fn replacement_gap_docs_cover_deploy_connectors_virtual_sandbox_and_migration() {
    let root = std::env::current_dir()
        .unwrap()
        .join("../..")
        .canonicalize()
        .unwrap();
    let docs = [
        (
            "docs/deploy-node.md".to_string(),
            vec![
                "agentic-harness build --target node".to_string(),
                "node server.mjs".to_string(),
            ],
        ),
        (
            "docs/deploy-cloudflare.md".to_string(),
            vec![
                "wrangler deploy".to_string(),
                "agentic_harness_app.js".to_string(),
            ],
        ),
        (
            "docs/deploy-github-actions.md".to_string(),
            vec![
                "agentic-harness run".to_string(),
                "GITHUB_TOKEN".to_string(),
            ],
        ),
        (
            "docs/deploy-gitlab-ci.md".to_string(),
            vec![
                "agentic-harness run".to_string(),
                "CI_JOB_TOKEN".to_string(),
            ],
        ),
        (
            "docs/connectors.md".to_string(),
            vec![
                "SandboxConnector::vercel".to_string(),
                "HttpSessionEnv".to_string(),
            ],
        ),
        (
            "docs/virtual-sandbox.md".to_string(),
            vec!["VirtualSessionEnv".to_string(), "hostless".to_string()],
        ),
        (
            ["docs/", "fl", "ue", "-migration.md"].concat(),
            vec![
                ["Fl", "ueContext"].concat(),
                "AgentContext".to_string(),
                ["fl", "ue add"].concat(),
            ],
        ),
    ];

    for (path, needles) in docs {
        let body = fs::read_to_string(root.join(&path)).unwrap_or_else(|err| {
            panic!("missing {path}: {err}");
        });
        for needle in needles {
            assert!(body.contains(&needle), "{path} should document {needle}");
        }
    }
}

#[test]
fn cloudflare_build_can_link_a_worker_app_adapter() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let output_dir = temp.path().join("out");
    let adapter_path = temp.path().join("worker_app.js");
    fs::write(
        &adapter_path,
        "export async function initAgenticHarnessApp() { globalThis.adapterLoaded = true; }\n\
         export async function invokeAgenticHarnessAgent(context) { return { agent: context.agentName, id: context.id, linked: true }; }\n",
    )
    .unwrap();

    let build = Command::new(agentic_harness_bin())
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "cloudflare",
            "--output",
            output_dir.to_str().unwrap(),
            "--worker-app",
            adapter_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let app_adapter = fs::read_to_string(output_dir.join("dist/agentic_harness_app.js")).unwrap();
    assert!(app_adapter.contains("adapterLoaded"));
    assert!(app_adapter.contains("linked: true"));
    assert!(!app_adapter.contains("handler_not_linked"));
}

#[test]
fn cloudflare_build_can_package_a_worker_wasm_module() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let output_dir = temp.path().join("out");
    let adapter_path = temp.path().join("worker_app.js");
    let wasm_path = temp.path().join("worker_app.wasm");
    fs::write(
        &adapter_path,
        "import wasm from './agentic_harness_app.wasm';\n\
         export async function initAgenticHarnessApp() { return wasm; }\n\
         export async function invokeAgenticHarnessAgent(context) { return { agent: context.agentName, wasm: true }; }\n",
    )
    .unwrap();
    fs::write(&wasm_path, b"\0asm-agentic-harness-test").unwrap();

    let build = Command::new(agentic_harness_bin())
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "cloudflare",
            "--output",
            output_dir.to_str().unwrap(),
            "--worker-app",
            adapter_path.to_str().unwrap(),
            "--worker-wasm",
            wasm_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let dist = output_dir.join("dist");
    let app_adapter = fs::read_to_string(dist.join("agentic_harness_app.js")).unwrap();
    let wasm = fs::read(dist.join("agentic_harness_app.wasm")).unwrap();

    assert!(app_adapter.contains("agentic_harness_app.wasm"));
    assert_eq!(wasm, b"\0asm-agentic-harness-test");
    assert!(!dist.join("agentic-harness-agent").exists());
}

#[test]
fn cloudflare_build_generates_default_wasm_adapter_when_only_wasm_is_provided() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let output_dir = temp.path().join("out");
    let wasm_path = temp.path().join("worker_app.wasm");
    fs::write(&wasm_path, b"\0asm-default-adapter").unwrap();

    let build = Command::new(agentic_harness_bin())
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "cloudflare",
            "--output",
            output_dir.to_str().unwrap(),
            "--worker-wasm",
            wasm_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let dist = output_dir.join("dist");
    let app_adapter = fs::read_to_string(dist.join("agentic_harness_app.js")).unwrap();
    let wasm = fs::read(dist.join("agentic_harness_app.wasm")).unwrap();

    assert!(app_adapter.contains("import wasmModule from './agentic_harness_app.wasm';"));
    assert!(app_adapter.contains("WebAssembly.instantiate"));
    assert!(app_adapter.contains("agentic_harness_invoke"));
    assert!(app_adapter.contains("agentic_harness_alloc"));
    assert!(app_adapter.contains("response.error"));
    assert!(app_adapter.contains("return response.result"));
    assert!(app_adapter.contains("Agentic Harness WASM adapter"));
    assert!(!app_adapter.contains("handler_not_linked"));
    assert_eq!(wasm, b"\0asm-default-adapter");
}

#[test]
fn cloudflare_build_can_compile_and_package_a_worker_wasm_crate() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let output_dir = temp.path().join("out");
    let wasm_crate = temp.path().join("worker-adapter");
    fs::create_dir_all(wasm_crate.join("src")).unwrap();
    fs::write(
        wasm_crate.join("Cargo.toml"),
        "[package]\nname = \"worker-adapter\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n",
    )
    .unwrap();
    fs::write(
        wasm_crate.join("src/lib.rs"),
        "pub extern \"C\" fn noop() {}\n",
    )
    .unwrap();

    let real_cargo = option_env!("CARGO").unwrap_or("cargo");
    let fake_bin = temp.path().join("fake-bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let fake_cargo = fake_bin.join("cargo");
    fs::write(
        &fake_cargo,
        format!(
            "#!/bin/sh\n\
             for arg in \"$@\"; do\n\
               if [ \"$arg\" = \"wasm32-unknown-unknown\" ]; then\n\
                 target_dir=\"\"\n\
                 prev=\"\"\n\
                 for inner in \"$@\"; do\n\
                   if [ \"$prev\" = \"--target-dir\" ]; then target_dir=\"$inner\"; fi\n\
                   prev=\"$inner\"\n\
                 done\n\
                 mkdir -p \"$target_dir/wasm32-unknown-unknown/release\"\n\
                 printf '\\0asm-compiled-by-fake-cargo' > \"$target_dir/wasm32-unknown-unknown/release/worker_adapter.wasm\"\n\
                 exit 0\n\
               fi\n\
             done\n\
             exec {real_cargo} \"$@\"\n"
        ),
    )
    .unwrap();
    let mut permissions = fs::metadata(&fake_cargo).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_cargo, permissions).unwrap();
    let old_path = std::env::var_os("PATH").unwrap_or_default();
    let path = std::env::join_paths(
        std::iter::once(fake_bin.clone()).chain(std::env::split_paths(&old_path)),
    )
    .unwrap();

    let build = Command::new(agentic_harness_bin())
        .env("PATH", path)
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--target",
            "cloudflare",
            "--output",
            output_dir.to_str().unwrap(),
            "--worker-wasm-crate",
            wasm_crate.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        build.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&build.stderr)
    );
    let dist = output_dir.join("dist");
    let app_adapter = fs::read_to_string(dist.join("agentic_harness_app.js")).unwrap();
    let wasm = fs::read(dist.join("agentic_harness_app.wasm")).unwrap();

    assert!(app_adapter.contains("agentic_harness_invoke"));
    assert_eq!(wasm, b"\0asm-compiled-by-fake-cargo");
}

#[test]
fn build_produces_native_dist_artifacts() {
    let temp = tempfile::tempdir().unwrap();
    let workspace = std::env::current_dir()
        .unwrap()
        .join("../../examples/hello-world")
        .canonicalize()
        .unwrap();
    let output_dir = temp.path().join("out");

    let output = Command::new(agentic_harness_bin())
        .args([
            "build",
            "--workspace",
            workspace.to_str().unwrap(),
            "--output",
            output_dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let manifest_path = output_dir.join("dist/manifest.json");
    let binary_path = output_dir.join("dist/agentic-harness-agent");
    assert!(
        manifest_path.exists(),
        "missing {}",
        manifest_path.display()
    );
    assert!(binary_path.exists(), "missing {}", binary_path.display());

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["agents"][0]["name"], "assistant");
}

#[test]
fn add_lists_and_prints_native_connector_instructions() {
    let list = Command::new(agentic_harness_bin())
        .arg("add")
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("agentic-harness add model"));
    assert!(list_text.contains("agentic-harness add sandbox"));
    assert!(list_text.contains("agentic-harness add mcp"));
    assert!(list_text.contains("agentic-harness add daytona"));
    assert!(list_text.contains("agentic-harness add vercel"));
    assert!(list_text.contains("agentic-harness add e2b"));
    assert!(list_text.contains("https://daytona.io"));
    assert!(list_text.contains("https://vercel.com"));
    assert!(list_text.contains("https://e2b.dev"));
    assert!(list_text.contains("Hosted coding sandbox target"));
    assert!(list_text.contains("sandbox"));
    assert!(list_text.contains("agentic-harness add <url> --category sandbox"));
    assert!(list_text.contains("Build a sandbox connector from scratch"));

    let printed = Command::new(agentic_harness_bin())
        .args(["add", "model", "--print"])
        .output()
        .unwrap();
    assert!(
        printed.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&printed.stderr)
    );
    let body = String::from_utf8_lossy(&printed.stdout);
    assert!(body.contains("ModelClient"));
    assert!(body.contains("Agentic Harness"));

    let daytona = Command::new(agentic_harness_bin())
        .args(["add", "daytona", "--print"])
        .output()
        .unwrap();
    assert!(
        daytona.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&daytona.stderr)
    );
    let daytona_body = String::from_utf8_lossy(&daytona.stdout);
    assert!(daytona_body.contains("Rust-native Daytona sandbox connector"));
    assert!(daytona_body.contains("SessionEnv"));
    assert!(daytona_body.contains("shell_with_options"));

    let e2b = Command::new(agentic_harness_bin())
        .args(["add", "e2b", "--print"])
        .output()
        .unwrap();
    assert!(
        e2b.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&e2b.stderr)
    );
    let e2b_body = String::from_utf8_lossy(&e2b.stdout);
    assert!(e2b_body.contains("Rust-native E2B sandbox connector"));
    assert!(e2b_body.contains("SessionEnv"));
    assert!(e2b_body.contains("ShellOptions::cwd"));

    let mcp = Command::new(agentic_harness_bin())
        .args(["add", "mcp", "--print"])
        .output()
        .unwrap();
    assert!(
        mcp.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&mcp.stderr)
    );
    let mcp_body = String::from_utf8_lossy(&mcp.stdout);
    assert!(mcp_body.contains("connect_mcp_server"));
    assert!(mcp_body.contains("McpServerOptions"));

    let alias = Command::new(agentic_harness_bin())
        .args(["add", "@VERCEL/SANDBOX", "--print"])
        .output()
        .unwrap();
    assert!(
        alias.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&alias.stderr)
    );
    let alias_body = String::from_utf8_lossy(&alias.stdout);
    assert!(alias_body.contains("Rust-native Vercel Sandbox connector"));
    assert!(alias_body.contains("first-class hosted coding target"));
    assert!(alias_body.contains("Create or reuse a Vercel Sandbox"));
    assert!(alias_body.contains("SandboxConnector::vercel"));
    assert!(alias_body.contains("try_session_with_id_and_env(\"project\", env)"));

    let piped = Command::new(agentic_harness_bin())
        .args(["add", "daytona"])
        .output()
        .unwrap();
    assert!(
        piped.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&piped.stderr)
    );
    let piped_body = String::from_utf8_lossy(&piped.stdout);
    assert!(piped_body.contains("Rust-native Daytona sandbox connector"));
    assert!(piped_body.contains("SessionEnv"));

    let human = Command::new(agentic_harness_bin())
        .args(["add", "daytona"])
        .env("AGENTIC_HARNESS_FORCE_HUMAN_ADD_HINT", "1")
        .output()
        .unwrap();
    let hint = String::from_utf8_lossy(&human.stderr);
    assert!(hint.contains("agentic-harness add daytona --print | codex"));
    assert!(hint.contains("agentic-harness add daytona --print | claude"));
    assert!(hint.contains("agentic-harness add daytona --print | opencode"));
    assert!(hint.contains("agentic-harness add daytona --print | pi"));
}

#[test]
fn add_category_roots_emit_agent_ready_connector_instructions() {
    let generic = Command::new(agentic_harness_bin())
        .args([
            "add",
            "https://e2b.dev/docs",
            "--category",
            "sandbox",
            "--print",
        ])
        .output()
        .unwrap();
    assert!(
        generic.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&generic.stderr)
    );
    let body = String::from_utf8_lossy(&generic.stdout);
    assert!(body.contains("https://e2b.dev/docs"));
    assert!(body.contains("SessionEnv"));
    assert!(body.contains("ShellOptions"));
    assert!(body.contains("Never invent API keys"));

    let piped_generic = Command::new(agentic_harness_bin())
        .args(["add", "https://e2b.dev/docs", "--category", "sandbox"])
        .output()
        .unwrap();
    assert!(
        piped_generic.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&piped_generic.stderr)
    );
    let piped_body = String::from_utf8_lossy(&piped_generic.stdout);
    assert!(piped_body.contains("https://e2b.dev/docs"));
    assert!(piped_body.contains("Build a Rust type that implements"));

    let unknown = Command::new(agentic_harness_bin())
        .args([
            "add",
            "https://example.com/docs",
            "--category",
            "billing",
            "--print",
        ])
        .output()
        .unwrap();
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("Unknown category \"billing\""));
}

#[test]
fn release_packaging_artifacts_are_locally_smoke_testable() {
    let root = std::env::current_dir().unwrap().join("../..");

    let version = Command::new(agentic_harness_bin())
        .arg("--version")
        .output()
        .unwrap();
    assert!(
        version.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&version.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&version.stdout).trim(),
        "agentic-harness 0.1.0"
    );

    let install_script = root.join("scripts/install.sh");
    let formula = root.join("Formula/agentic-harness.rb");
    let changelog = root.join("CHANGELOG.md");
    let smoke_doc = root.join("docs/release-smoke-test.md");

    assert!(
        install_script.exists(),
        "missing {}",
        install_script.display()
    );
    assert!(formula.exists(), "missing {}", formula.display());
    assert!(changelog.exists(), "missing {}", changelog.display());
    assert!(smoke_doc.exists(), "missing {}", smoke_doc.display());
    assert!(
        fs::metadata(&install_script).unwrap().permissions().mode() & 0o111 != 0,
        "{} must be executable",
        install_script.display()
    );

    let install_body = fs::read_to_string(&install_script).unwrap();
    assert!(install_body.contains("cargo install --path"));
    assert!(install_body.contains("agentic-harness"));
    assert!(install_body.contains("--version"));
    let install_check = Command::new("bash")
        .args(["-n", install_script.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        install_check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install_check.stderr)
    );

    let formula_body = fs::read_to_string(&formula).unwrap();
    assert!(formula_body.contains("class AgenticHarness < Formula"));
    assert!(formula_body.contains("license \"Apache-2.0\""));
    assert!(formula_body.contains("agentic-harness --version"));
    assert!(formula_body.contains("\"cargo\""));
    assert!(formula_body.contains("\"install\""));
    let formula_check = Command::new("ruby")
        .args(["-c", formula.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        formula_check.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&formula_check.stderr)
    );

    let changelog_body = fs::read_to_string(&changelog).unwrap();
    assert!(changelog_body.contains("## 0.1.0"));
    assert!(changelog_body.contains("Native Rust"));

    let smoke_body = fs::read_to_string(&smoke_doc).unwrap();
    assert!(smoke_body.contains("agentic-harness --version"));
    assert!(smoke_body.contains("agentic-harness smoke --json"));
    assert!(smoke_body.contains("agentic-harness release-check --json"));
    assert!(smoke_body.contains("agentic-harness tui --plain"));
    assert!(smoke_body.contains("cargo test --workspace"));
}

#[test]
fn release_check_reports_packaging_readiness_as_text_and_json() {
    let root = std::env::current_dir().unwrap().join("../..");

    let text = Command::new(agentic_harness_bin())
        .args(["release-check", "--root", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    let body = String::from_utf8_lossy(&text.stdout);
    assert!(body.contains("Agentic Harness Release Check"));
    assert!(body.contains("install script: ok"));
    assert!(body.contains("homebrew formula: ok"));
    assert!(body.contains("changelog: ok"));
    assert!(body.contains("binary package: ok"));
    assert!(body.contains("release smoke doc: ok"));
    assert!(body.contains("cargo test --workspace"));

    let json = Command::new(agentic_harness_bin())
        .args(["release-check", "--root", root.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], "0.1.0");
    assert_eq!(body["checks"]["installScript"]["ok"], true);
    assert_eq!(body["checks"]["homebrewFormula"]["ok"], true);
    assert_eq!(body["checks"]["changelog"]["ok"], true);
    assert_eq!(body["checks"]["binaryPackage"]["ok"], true);
    assert_eq!(body["checks"]["releaseSmokeDoc"]["ok"], true);
    assert!(body["nextCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command == "cargo clippy --workspace -- -D warnings"));
    assert!(body["nextCommands"]
        .as_array()
        .unwrap()
        .iter()
        .any(|command| command == "agentic-harness package --output dist/packages --json"));
}

#[test]
fn release_check_rejects_formula_with_invalid_ruby_syntax() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("scripts")).unwrap();
    fs::create_dir_all(root.join("Formula")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();

    let install_script = root.join("scripts/install.sh");
    fs::write(
        &install_script,
        "#!/usr/bin/env bash\ncargo install --path crates/agentic-harness-cli --root \"$HOME/.agentic-harness\" --locked --force\nagentic-harness --version\n",
    )
    .unwrap();
    let mut permissions = fs::metadata(&install_script).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&install_script, permissions).unwrap();

    fs::write(
        root.join("Formula/agentic-harness.rb"),
        "class AgenticHarness < Formula\n  license \"Apache-2.0\"\n  def install(\n    system \"cargo\", \"install\", \"--path\", \"crates/agentic-harness-cli\"\n  end\n  test do\n    shell_output(\"#{bin}/agentic-harness --version\")\n  end\nend\n",
    )
    .unwrap();
    fs::write(
        root.join("CHANGELOG.md"),
        "## 0.1.0\n\n- Native Rust runtime.\n- Release packaging binary package artifacts.\n",
    )
    .unwrap();
    fs::write(
        root.join("README.md"),
        "Run `agentic-harness package --output dist/packages --json`.\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/release-smoke-test.md"),
        "agentic-harness --version\nagentic-harness smoke --json\nagentic-harness release-check --json\nagentic-harness tui --plain\nagentic-harness inspect --workspace .\ncargo test --workspace\ncargo clippy --workspace -- -D warnings\nagentic-harness package --output dist/packages --json\n",
    )
    .unwrap();

    let output = Command::new(agentic_harness_bin())
        .args(["release-check", "--root", root.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["checks"]["homebrewFormula"]["ok"], false);
    assert!(body["checks"]["homebrewFormula"]["detail"]
        .as_str()
        .unwrap()
        .contains("ruby syntax check failed"));
}

#[test]
fn package_command_creates_binary_distribution_manifest_and_checksums() {
    let temp = tempfile::tempdir().unwrap();
    let output_dir = temp.path().join("packages");

    let text = Command::new(agentic_harness_bin())
        .args(["package", "--output", output_dir.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    let body = String::from_utf8_lossy(&text.stdout);
    assert!(body.contains("Agentic Harness Package"));
    assert!(body.contains("version: 0.1.0"));
    assert!(body.contains("sha256:"));

    let package_dir = output_dir.join(format!(
        "agentic-harness-v0.1.0-{}-{}",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let binary = package_dir.join("agentic-harness");
    let manifest = package_dir.join("manifest.json");
    let checksums = package_dir.join("SHA256SUMS");
    let archive = output_dir.join(format!(
        "agentic-harness-v0.1.0-{}-{}.tar",
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    assert!(binary.exists(), "missing {}", binary.display());
    assert!(manifest.exists(), "missing {}", manifest.display());
    assert!(checksums.exists(), "missing {}", checksums.display());
    assert!(archive.exists(), "missing {}", archive.display());
    assert!(
        fs::metadata(&binary).unwrap().permissions().mode() & 0o111 != 0,
        "binary is not executable"
    );

    let manifest: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(manifest).unwrap()).unwrap();
    assert_eq!(manifest["name"], "agentic-harness");
    assert_eq!(manifest["version"], "0.1.0");
    assert_eq!(manifest["platform"]["os"], std::env::consts::OS);
    assert_eq!(manifest["platform"]["arch"], std::env::consts::ARCH);
    assert_eq!(manifest["binary"], "agentic-harness");
    assert_eq!(
        manifest["archive"],
        format!(
            "agentic-harness-v0.1.0-{}-{}.tar",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
    );
    assert_eq!(manifest["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(manifest["archiveSha256"].as_str().unwrap().len(), 64);
    assert!(manifest["sizeBytes"].as_u64().unwrap() > 0);
    assert!(manifest["archiveSizeBytes"].as_u64().unwrap() > 0);

    let sums = fs::read_to_string(checksums).unwrap();
    assert!(sums.contains(manifest["sha256"].as_str().unwrap()));
    assert!(sums.contains(manifest["archiveSha256"].as_str().unwrap()));
    assert!(sums.contains("agentic-harness"));
    assert!(sums.contains(".tar"));
    let archive_entries = read_tar_entries(&archive);
    let archive_names = archive_entries
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<Vec<_>>();
    assert!(archive_names.contains(
        &format!(
            "agentic-harness-v0.1.0-{}-{}/agentic-harness",
            std::env::consts::OS,
            std::env::consts::ARCH
        )
        .as_str()
    ));
    let archived_manifest = archive_entries
        .iter()
        .find(|(name, _)| {
            name == &format!(
                "agentic-harness-v0.1.0-{}-{}/manifest.json",
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        })
        .expect("archive missing manifest.json");
    let archived_manifest: serde_json::Value =
        serde_json::from_slice(&archived_manifest.1).unwrap();
    assert_eq!(archived_manifest["version"], "0.1.0");
    assert_eq!(archived_manifest["binary"], "agentic-harness");
    assert_eq!(archived_manifest["sha256"], manifest["sha256"]);

    let json = Command::new(agentic_harness_bin())
        .args([
            "package",
            "--output",
            output_dir.to_str().unwrap(),
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["artifact"]["version"], "0.1.0");
    assert_eq!(body["artifact"]["sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        body["artifact"]["archiveSha256"].as_str().unwrap().len(),
        64
    );
}

#[test]
fn smoke_command_reports_install_readiness_as_text_and_json() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("smoke-agent");
    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "smoke-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let text = Command::new(agentic_harness_bin())
        .args(["smoke", "--workspace", project.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        text.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&text.stderr)
    );
    let body = String::from_utf8_lossy(&text.stdout);
    assert!(body.contains("Agentic Harness Smoke"));
    assert!(body.contains("version: agentic-harness 0.1.0"));
    assert!(body.contains("wizard: ok"));
    assert!(body.contains("doctor: ok"));
    assert!(body.contains("sandbox: ok"));

    let json = Command::new(agentic_harness_bin())
        .args(["smoke", "--workspace", project.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        json.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&json.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(body["ok"], true);
    assert_eq!(body["version"], "agentic-harness 0.1.0");
    assert_eq!(
        body["workspace"],
        project.canonicalize().unwrap().display().to_string()
    );
    assert_eq!(body["checks"]["wizardPlain"], true);
    assert_eq!(body["checks"]["doctorRequiredReady"], true);
    assert_eq!(body["checks"]["sandboxSmoke"]["ok"], true);
    assert!(body["checks"]["llmTools"]["available"].is_boolean());
    assert_eq!(
        body["checks"]["llmTools"]["tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(body["checks"]["llmTools"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["environment"] == "codex" && tool["command"] == "codex"));
    assert!(body["nextCommand"]
        .as_str()
        .unwrap()
        .starts_with("agentic-harness code --workspace"));
}

#[test]
fn smoke_reports_llm_tool_readiness_separately_from_presence() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("llm-readiness-agent");

    let scaffold = Command::new(agentic_harness_bin())
        .args([
            "new",
            project.to_str().unwrap(),
            "--name",
            "llm-readiness-agent",
            "--template",
            "coding",
        ])
        .output()
        .unwrap();
    assert!(
        scaffold.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&scaffold.stderr)
    );

    let fake_bin = temp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    for (name, script) in [
        (
            "claude",
            r#"#!/bin/sh
if [ "$1" = "auth" ] && [ "$2" = "status" ]; then
  printf '{"loggedIn":false,"authMethod":"none"}\n'
  exit 1
fi
exit 0
"#,
        ),
        (
            "codex",
            r#"#!/bin/sh
if [ "$1" = "login" ] && [ "$2" = "status" ]; then
  printf 'Logged in using ChatGPT\n'
  exit 0
fi
exit 0
"#,
        ),
        (
            "cursor",
            r#"#!/bin/sh
if [ "$1" = "agent" ] && [ "$2" = "status" ]; then
  printf '{"authenticated":false}\n'
  exit 1
fi
exit 0
"#,
        ),
    ] {
        let path = fake_bin.join(name);
        fs::write(&path, script).unwrap();
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&path, permissions).unwrap();
    }

    let output = Command::new(agentic_harness_bin())
        .env(
            "PATH",
            format!(
                "{}:{}",
                fake_bin.display(),
                std::env::var("PATH").unwrap_or_default()
            ),
        )
        .args(["smoke", "--workspace", project.to_str().unwrap(), "--json"])
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let tools = body["checks"]["llmTools"]["tools"].as_array().unwrap();
    assert!(tools.iter().any(|tool| tool["environment"] == "claude-code"
        && tool["found"] == true
        && tool["ready"] == false
        && tool["detail"].as_str().unwrap().contains("login")));
    assert!(tools.iter().any(|tool| tool["environment"] == "codex"
        && tool["found"] == true
        && tool["ready"] == true
        && tool["detail"].as_str().unwrap().contains("ready")));
    assert!(tools.iter().any(|tool| tool["environment"] == "cursor"
        && tool["found"] == true
        && tool["ready"] == false));
    assert!(tools.iter().any(|tool| tool["environment"] == "wind-server"
        && tool["found"] == false
        && tool["ready"] == false));
    assert_eq!(body["checks"]["llmTools"]["available"], true);
}
