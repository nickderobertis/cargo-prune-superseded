//! The release wiring, read offline: `.github/workflows/release-plz.yml` runs release-plz
//! on pushes to the default branch with the committed `release-plz.toml`, authenticated
//! by the release personal access token (a tag or pull request `GITHUB_TOKEN` creates
//! triggers no other workflow) and handed the crates.io token; and the one package it
//! publishes is the one the `crate` target in `release-targets.toml` declares.
//!
//! These assertions are structural by necessity: running the workflow for real publishes
//! to crates.io, tags and cuts a release in the live repository with secrets this suite
//! never holds, and none of that can be undone. The pipeline short of publishing is
//! driven by hand with `release-plz update` and `release-plz release --dry-run` against
//! this same configuration; these checks hold the wiring that run depends on.

#![cfg(unix)]

use std::path::{Path, PathBuf};

use serde_norway::Value;

const RELEASE_TOKEN: &str = "${{ secrets.RELEASE_PLZ_TOKEN }}";
const CRATES_IO_TOKEN: &str = "${{ secrets.CARGO_REGISTRY_TOKEN }}";
const CONFIG: &str = "release-plz.toml";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(relative: &str) -> String {
    std::fs::read_to_string(root().join(relative))
        .unwrap_or_else(|error| panic!("{relative} must be readable: {error}"))
}

fn workflow() -> Value {
    serde_norway::from_str(&read(".github/workflows/release-plz.yml"))
        .expect("the release workflow is valid YAML")
}

fn toml_file(relative: &str) -> toml::Table {
    read(relative)
        .parse()
        .unwrap_or_else(|error| panic!("{relative} must be valid TOML: {error}"))
}

/// Every step of every job, in order.
fn steps(workflow: &Value) -> Vec<&Value> {
    workflow["jobs"]
        .as_mapping()
        .expect("the workflow declares jobs")
        .values()
        .flat_map(|job| job["steps"].as_sequence().into_iter().flatten())
        .collect()
}

fn run_of(step: &Value) -> &str {
    step["run"].as_str().unwrap_or_default()
}

fn env_of<'a>(step: &'a Value, name: &str) -> Option<&'a str> {
    step["env"][name].as_str()
}

/// The steps whose command runs `release-plz <subcommand>` (and not a longer one).
fn release_plz_steps<'a>(workflow: &'a Value, subcommand: &str) -> Vec<&'a Value> {
    steps(workflow)
        .into_iter()
        .filter(|step| {
            run_of(step)
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|pair| pair[0] == "release-plz" && pair[1] == subcommand)
        })
        .collect()
}

#[test]
fn the_workflow_runs_on_pushes_to_the_default_branch() {
    let workflow = workflow();
    let branches = workflow["on"]["push"]["branches"]
        .as_sequence()
        .expect("the workflow triggers on push with a branch list");
    assert!(
        branches
            .iter()
            .any(|branch| branch.as_str() == Some("main")),
        "the release workflow must run on pushes to main, got {branches:?}"
    );
}

#[test]
fn release_plz_publishes_with_the_committed_configuration() {
    let workflow = workflow();
    let release = release_plz_steps(&workflow, "release");
    assert_eq!(
        release.len(),
        1,
        "exactly one step runs `release-plz release`"
    );
    assert!(
        root().join(CONFIG).is_file(),
        "{CONFIG} must be committed at the repository root"
    );
    for step in release
        .iter()
        .chain(&release_plz_steps(&workflow, "release-pr"))
    {
        assert!(
            run_of(step).contains(&format!("--config {CONFIG}")),
            "every release-plz invocation must name --config {CONFIG}: {}",
            run_of(step)
        );
    }
}

#[test]
fn release_plz_authenticates_with_the_personal_access_token() {
    let workflow = workflow();
    let invocations: Vec<_> = release_plz_steps(&workflow, "release")
        .into_iter()
        .chain(release_plz_steps(&workflow, "release-pr"))
        .collect();
    assert_eq!(
        invocations.len(),
        2,
        "release-plz releases and opens release PRs"
    );
    for step in invocations {
        assert_eq!(
            env_of(step, "GITHUB_TOKEN"),
            Some(RELEASE_TOKEN),
            "release-plz must be given the release PAT, not GITHUB_TOKEN"
        );
        assert!(
            run_of(step).contains("--git-token \"$GITHUB_TOKEN\""),
            "release-plz must be handed that token explicitly: {}",
            run_of(step)
        );
    }
    let checkout = steps(&workflow)
        .into_iter()
        .find(|step| {
            step["uses"]
                .as_str()
                .is_some_and(|uses| uses.starts_with("actions/checkout@"))
        })
        .expect("the workflow checks the repository out");
    assert_eq!(
        checkout["with"]["token"].as_str(),
        Some(RELEASE_TOKEN),
        "the checkout must use the release PAT, so the tag it pushes triggers workflows"
    );
    let text = read(".github/workflows/release-plz.yml");
    for default_token in ["secrets.GITHUB_TOKEN", "github.token"] {
        assert!(
            !text.contains(default_token),
            "the release workflow must not authenticate with {default_token}"
        );
    }
}

#[test]
fn release_plz_is_handed_the_crates_io_token() {
    let workflow = workflow();
    let release = release_plz_steps(&workflow, "release");
    assert_eq!(
        release
            .first()
            .and_then(|step| env_of(step, "CARGO_REGISTRY_TOKEN")),
        Some(CRATES_IO_TOKEN),
        "`release-plz release` must get CARGO_REGISTRY_TOKEN from its repository secret"
    );
}

#[test]
fn only_a_merged_release_pull_request_publishes() {
    let workflow = workflow();
    let release = release_plz_steps(&workflow, "release");
    assert_eq!(
        release.first().and_then(|step| step["if"].as_str()),
        Some("steps.release-pr.outputs.merged == 'true'"),
        "`release-plz release` must run only for a merged release pull request"
    );
    let detect = steps(&workflow)
        .into_iter()
        .find(|step| step["id"].as_str() == Some("release-pr"))
        .expect("a step with id release-pr decides whether the push is a release");
    let run = run_of(detect);
    assert!(
        run.contains("/commits/$GITHUB_SHA/pulls")
            && run.contains("merged_at != null")
            && run.contains("release-plz-*) echo \"merged=true\""),
        "release-pr must find the merged pull request of the pushed commit and accept only \
         a release-plz-* head branch: {run}"
    );
    assert_eq!(env_of(detect, "GH_TOKEN"), Some(RELEASE_TOKEN));
}

#[test]
fn the_published_package_is_the_declared_crate_target() {
    let config = toml_file(CONFIG);
    let packages = config["package"]
        .as_array()
        .expect("release-plz.toml lists packages");
    let released = |package: &&toml::Value| {
        package.get("release").and_then(toml::Value::as_bool) != Some(false)
            && package.get("publish").and_then(toml::Value::as_bool) != Some(false)
    };
    let published: Vec<&str> = packages
        .iter()
        .filter(released)
        .filter_map(|package| package.get("name").and_then(toml::Value::as_str))
        .collect();
    assert_eq!(
        published,
        ["cargo-prune-superseded"],
        "release-plz publishes one package"
    );

    // Every other workspace member is named, unreleased, so nothing else publishes.
    let workspace = toml_file("Cargo.toml");
    let members = workspace["workspace"]["members"]
        .as_array()
        .expect("workspace members");
    for member in members {
        let dir = member.as_str().expect("a member path");
        let manifest = toml_file(&format!("{dir}/Cargo.toml"));
        let name = manifest["package"]["name"]
            .as_str()
            .expect("a package name");
        if name != published[0] {
            assert!(
                packages.iter().any(|package| {
                    package.get("name").and_then(toml::Value::as_str) == Some(name)
                        && package.get("release").and_then(toml::Value::as_bool) == Some(false)
                        && package.get("publish").and_then(toml::Value::as_bool) == Some(false)
                }),
                "{name} must be `release = false` and `publish = false` in {CONFIG}"
            );
            assert_eq!(
                manifest["package"]
                    .get("publish")
                    .and_then(toml::Value::as_bool),
                Some(false),
                "{name} must be `publish = false`"
            );
        }
    }

    let declaration = toml_file("release-targets.toml");
    let targets = declaration["target"].as_array().expect("[[target]] tables");
    assert_eq!(targets.len(), 1, "exactly one release target is declared");
    let target = &targets[0];
    assert_eq!(target["name"].as_str(), Some("crate"));
    assert_eq!(target["id"].as_str(), Some("crate:cargo-prune-superseded"));
    let manifest = toml_file(target["manifest"].as_str().expect("the target's manifest"));
    let declared = manifest["package"]["name"]
        .as_str()
        .expect("a package name");
    assert_eq!(
        declared, published[0],
        "the crate target's manifest is the published package"
    );
    assert_eq!(
        target["id"].as_str(),
        Some(format!("crate:{declared}").as_str())
    );
}

/// release-plz names its release pull request's branch `release-plz-…` unless
/// `pr_branch_prefix` says otherwise; CI's tier selection and the release workflow's
/// publication check must both key on that one prefix.
#[test]
fn the_release_branch_prefix_is_one_convention() {
    let config = toml_file(CONFIG);
    let prefix = config["workspace"]
        .get("pr_branch_prefix")
        .and_then(toml::Value::as_str)
        .unwrap_or("release-plz-");
    let ci = read(".github/workflows/ci.yml");
    let selector = format!("startsWith(github.head_ref, '{prefix}')");
    assert_eq!(
        ci.matches(&selector).count(),
        2,
        "ci.yml's gate and check-all must both select on {selector}"
    );
    assert!(
        read(".github/workflows/release-plz.yml")
            .contains(&format!("{prefix}*) echo \"merged=true\"")),
        "the release workflow must recognise a merged {prefix}* pull request"
    );
}

#[test]
fn the_declared_probe_is_the_script_the_probe_suite_drives() {
    use std::os::unix::fs::PermissionsExt;

    let declaration = toml_file("release-targets.toml");
    assert_eq!(
        declaration["probe"].as_str(),
        Some("scripts/release-probe.sh"),
        "release-targets.toml must declare the probe tests/release_probe.rs drives"
    );
    let mode = std::fs::metadata(root().join("scripts/release-probe.sh"))
        .expect("the probe exists")
        .permissions()
        .mode();
    assert_ne!(
        mode & 0o111,
        0,
        "the probe must be executable to be spawned directly"
    );
}
