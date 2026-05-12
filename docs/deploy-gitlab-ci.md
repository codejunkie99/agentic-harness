# Run in GitLab CI

Use GitLab CI for merge-request review, scheduled maintenance, or repository
checks where the project is already mounted in the runner.

This pattern is best for agents that can run entirely inside the checked-out
repository or inside a configured remote `SessionEnv` sandbox.

```yaml
agentic_harness:
  image: rust:latest
  stage: test
  script:
    - cargo install --path crates/agentic-harness-cli
    - |
      agentic-harness run triage \
        --workspace . \
        --id "pipeline-${CI_PIPELINE_ID}" \
        --payload "{\"project\":\"${CI_PROJECT_PATH}\",\"sha\":\"${CI_COMMIT_SHA}\"}"
  variables:
    AGENTIC_HARNESS_CI: "gitlab"
```

GitLab exposes `CI_JOB_TOKEN` to trusted job code. Pass it through explicit
command env or provider config only when the agent really needs it.

For merge-request automation, grant only the API scopes required by the action
the agent performs. Read-only review and reproduction jobs should not receive
write-capable tokens.
