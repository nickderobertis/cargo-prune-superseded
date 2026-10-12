//! The CI contexts branch protection requires, read offline: every context `just
//! governance` names is a job that reports on every pull request, the `check` context
//! aggregates every other gating job in the CI workflow, and the harness the llmlint job
//! installs is one `oneharness.toml` falls back to.

use std::path::{Path, PathBuf};

use serde_norway::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("{relative} must be readable: {error}"))
}

fn yaml(relative: &str) -> Value {
    serde_norway::from_str(&read(relative))
        .unwrap_or_else(|error| panic!("{relative} must be valid YAML: {error}"))
}

/// The required contexts the `governance` recipe passes to create-repo's script: the
/// words between `"$script"` and `"$@"` that are not flags.
fn required_contexts() -> Vec<String> {
    let justfile = read("justfile");
    let line = justfile
        .lines()
        .find(|line| line.contains("uv run") && line.contains("\"$script\""))
        .expect("the governance recipe runs the governance script");
    let after = line
        .split("\"$script\"")
        .nth(1)
        .expect("arguments after the script");
    let before = after
        .split("\"$@\"")
        .next()
        .expect("the contexts before \"$@\"");
    before
        .split_whitespace()
        .filter(|word| !word.starts_with('-'))
        .map(str::to_owned)
        .collect()
}

/// Every job of every workflow that runs on `pull_request`, by job id.
fn pull_request_jobs() -> Vec<(String, Value)> {
    let mut jobs = Vec::new();
    for entry in std::fs::read_dir(root().join(".github/workflows")).expect("the workflows") {
        let path = entry.expect("a workflow entry").path();
        let relative = format!(
            ".github/workflows/{}",
            path.file_name().expect("a file name").to_string_lossy()
        );
        let workflow = yaml(&relative);
        if workflow["on"].get("pull_request").is_none() {
            continue;
        }
        for (id, job) in workflow["jobs"].as_mapping().expect("jobs") {
            jobs.push((id.as_str().expect("a job id").to_owned(), job.clone()));
        }
    }
    jobs
}

#[test]
fn every_required_context_is_a_job_reporting_on_every_pull_request() {
    let contexts = required_contexts();
    assert_eq!(contexts, ["check", "pr-title", "llmlint"]);
    let jobs = pull_request_jobs();
    for context in &contexts {
        let (_, job) = jobs
            .iter()
            .find(|(id, _)| id == context)
            .unwrap_or_else(|| panic!("no pull_request job is named {context}"));
        assert!(
            job.get("strategy").is_none() && job.get("name").is_none(),
            "{context} must report under its job id, not a matrix or display name"
        );
        let condition = job["if"].as_str().unwrap_or("");
        assert!(
            condition.is_empty()
                || condition == "always()"
                || condition == "github.event_name == 'pull_request'",
            "{context} must run on every pull request, got if: {condition}"
        );
    }
}

#[test]
fn check_aggregates_every_other_gating_job() {
    let ci = yaml(".github/workflows/ci.yml");
    let jobs = ci["jobs"].as_mapping().expect("ci jobs");
    let needs: Vec<&str> = jobs[&Value::from("check")]["needs"]
        .as_sequence()
        .expect("check needs a list of jobs")
        .iter()
        .map(|need| need.as_str().expect("a job id"))
        .collect();
    let others: Vec<&str> = jobs
        .keys()
        .map(|id| id.as_str().expect("a job id"))
        .filter(|id| !["check", "llmlint"].contains(id))
        .collect();
    assert_eq!(
        needs, others,
        "check must need every other gating job in ci.yml"
    );
}

#[test]
fn the_llmlint_job_installs_a_harness_oneharness_falls_back_to() {
    let config: toml::Table = read("oneharness.toml").parse().expect("valid TOML");
    let harnesses: Vec<&str> = config["harnesses"]
        .as_array()
        .expect("a harnesses list")
        .iter()
        .filter_map(toml::Value::as_str)
        .collect();
    let ci = read(".github/workflows/ci.yml");
    // Each harness oneharness drives, and the npm package that installs it.
    let installed: Vec<&str> = [
        ("claude-code", "@anthropic-ai/claude-code"),
        ("codex", "@openai/codex"),
    ]
    .into_iter()
    .filter(|(_, package)| ci.contains(&format!("npm i -g {package}")))
    .map(|(harness, _)| harness)
    .collect();
    assert!(
        !installed.is_empty() && installed.iter().all(|harness| harnesses.contains(harness)),
        "the llmlint job must install a harness oneharness.toml lists ({harnesses:?}); installs {installed:?}"
    );
}
