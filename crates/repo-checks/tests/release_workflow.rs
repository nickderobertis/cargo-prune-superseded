//! The release wiring, read offline: `.github/workflows/release-plz.yml` runs release-plz
//! on pushes to the default branch with the committed `release-plz.toml`, authenticated
//! by the release personal access token (a tag or pull request `GITHUB_TOKEN` creates
//! triggers no other workflow) and handed the crates.io token; and the one package it
//! publishes is the one the `crate` target in `release-targets.toml` declares. The release
//! pull request arms auto-merge with that token, so it merges itself once its required
//! checks pass, and nothing merges past them.
//!
//! These assertions are structural by necessity: running the workflow for real publishes
//! to crates.io, tags and cuts a release in the live repository with secrets this suite
//! never holds, and none of that can be undone. Even `release-plz release --dry-run`
//! needs a forge token and the network, which this offline suite has neither of; these
//! checks hold the wiring that run depends on. The one step that needs no secret, arming
//! auto-merge, is also run: its own script under `bash`, with only `gh` doubled.

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

const ARMING_STEP: &str = "Auto-merge the release PR when the gate is green";

fn job_steps<'a>(workflow: &'a Value, job: &str) -> Vec<&'a Value> {
    workflow["jobs"][job]["steps"]
        .as_sequence()
        .map(|steps| steps.iter().collect())
        .unwrap_or_default()
}

/// Each `gh pr merge` command in a step's script (a fallback chained on the same line
/// included), as its words: continuation lines joined, cut at the first `||`, `&&`, `;` or
/// `|` that ends the command.
fn gh_pr_merges(run: &str) -> Vec<Vec<String>> {
    run.replace("\\\n", " ")
        .lines()
        .flat_map(|line| line.split("gh pr merge").skip(1))
        .map(|rest| {
            rest.split_whitespace()
                .take_while(|word| !matches!(*word, "||" | "&&" | ";" | "|"))
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

fn arming_step(workflow: &Value) -> &Value {
    let arming: Vec<_> = steps(workflow)
        .into_iter()
        .filter(|step| !gh_pr_merges(run_of(step)).is_empty())
        .collect();
    assert_eq!(
        arming.len(),
        1,
        "exactly one step runs `gh pr merge`, the one arming auto-merge on the release PR"
    );
    arming[0]
}

/// The release pull request merges itself once its required checks pass: one step, after
/// release-plz has opened or updated it in the same job, arms `gh pr merge --auto
/// --squash` on the open `release-plz-*` pull request with the release PAT (a merge the
/// default token makes pushes a commit that triggers no workflow, so nothing would
/// publish it). `--auto` is what keeps the merge behind the checks: no `gh pr merge` may
/// drop it or force past them with `--admin`.
#[test]
fn the_release_pull_request_arms_auto_merge_behind_its_checks() {
    let workflow = workflow();
    let step = arming_step(&workflow);
    assert_eq!(step["name"].as_str(), Some(ARMING_STEP));

    let release_pr_job = job_steps(&workflow, "release-pr");
    let position = |wanted: &Value| {
        release_pr_job
            .iter()
            .position(|step| std::ptr::eq(*step, wanted))
    };
    let opens = release_plz_steps(&workflow, "release-pr");
    assert_eq!(
        opens.len(),
        1,
        "exactly one step runs `release-plz release-pr`"
    );
    let opens_at = position(opens[0]).expect("`release-plz release-pr` runs in the release-pr job");
    let arms_at = position(step).expect("the arming step runs in the release-pr job");
    assert!(
        arms_at > opens_at,
        "auto-merge must be armed after release-plz opens or updates the release PR"
    );

    assert_eq!(
        env_of(step, "GITHUB_TOKEN"),
        Some(RELEASE_TOKEN),
        "gh must arm the merge with the release PAT, so the merge commit triggers the release"
    );
    let mapping = step["env"]
        .as_mapping()
        .expect("the arming step has an env");
    assert_eq!(
        mapping.len(),
        1,
        "the arming step is handed only the release PAT"
    );

    let run = run_of(step);
    assert!(
        run.contains("gh pr list --repo \"$GITHUB_REPOSITORY\" --state open")
            && run.contains("startswith(\"release-plz-\")"),
        "the arming step must select the open release-plz-* pull request: {run}"
    );
    for words in gh_pr_merges(run) {
        for flag in ["--auto", "--squash"] {
            assert!(
                words.iter().any(|word| word == flag),
                "the release PR's merge must pass {flag}: gh pr merge {words:?}"
            );
        }
    }
}

#[test]
fn no_merge_in_the_workflow_bypasses_the_required_checks() {
    let workflow = workflow();
    let mut merges = 0;
    for step in steps(&workflow) {
        for words in gh_pr_merges(run_of(step)) {
            merges += 1;
            assert!(
                words.iter().any(|word| word == "--auto"),
                "every gh pr merge must wait on the checks with --auto: gh pr merge {words:?}"
            );
            assert!(
                !words.iter().any(|word| word == "--admin"),
                "no gh pr merge may force past the checks with --admin: gh pr merge {words:?}"
            );
        }
    }
    assert!(merges > 0, "the workflow arms auto-merge on the release PR");
    for step in steps(&workflow) {
        assert!(
            !run_of(step).contains("--admin"),
            "no step in the release workflow may pass --admin: {}",
            run_of(step)
        );
    }
}

/// The directory holding the `gh` double put first on `PATH` (the only thing doubled): it
/// records each invocation's arguments and the token it was handed, answers `gh pr list` by
/// applying the caller's own `--jq` filter with the real `jq` to canned `--json
/// number,headRefName` output, and answers `gh pr merge` with the configured exit status.
/// It is checked in, executable, rather than written by the test: nextest runs each test in
/// its own process, and one process executing a double another is still writing fails with
/// ETXTBSY ("Text file busy").
fn gh_double_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/gh-double")
}

const REPOSITORY: &str = "nickderobertis/cargo-prune-superseded";
const TOKEN: &str = "release-pat-for-the-double";

struct Armed {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
    /// Each `gh` invocation: the token it saw, then its arguments.
    calls: Vec<Vec<String>>,
}

impl Armed {
    fn merges(&self) -> Vec<&[String]> {
        self.calls
            .iter()
            .filter(|call| call[1..].starts_with(&["pr".to_owned(), "merge".to_owned()]))
            .map(|call| &call[1..])
            .collect()
    }
}

/// Runs the arming step's own script, read from the real workflow, the way the runner's
/// default `bash -e` shell does, against `gh pr list` answering `pulls`.
fn run_arming_step(pulls: &str, merge_exit: u8) -> Armed {
    let jq = std::process::Command::new("jq").arg("--version").output();
    assert!(
        jq.is_ok_and(|output| output.status.success()),
        "jq must be installed: the gh double answers --jq with the real jq, as gh does"
    );
    let workflow = workflow();
    let script = run_of(arming_step(&workflow)).to_owned();
    let scratch = tempfile::tempdir().expect("a scratch directory");
    let script_path = scratch.path().join("step.sh");
    std::fs::write(&script_path, script).expect("the step's script");
    let pulls_path = scratch.path().join("pulls.json");
    std::fs::write(&pulls_path, pulls).expect("the canned pull requests");
    let calls_path = scratch.path().join("calls");
    let path = std::env::join_paths(
        std::iter::once(gh_double_dir()).chain(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        )),
    )
    .expect("a PATH");
    let output = std::process::Command::new("bash")
        .args(["--noprofile", "--norc", "-e"])
        .arg(&script_path)
        .env("PATH", path)
        .env("GITHUB_REPOSITORY", REPOSITORY)
        .env("GITHUB_TOKEN", TOKEN)
        .env("GH_DOUBLE_PULLS", &pulls_path)
        .env("GH_DOUBLE_CALLS", &calls_path)
        .env("GH_DOUBLE_MERGE_EXIT", merge_exit.to_string())
        .output()
        .expect("bash runs the arming step");
    let calls = std::fs::read_to_string(&calls_path)
        .unwrap_or_default()
        .lines()
        .map(|line| line.split('\x1f').map(str::to_owned).collect())
        .collect();
    Armed {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        calls,
    }
}

/// What `gh pr list --state open --json number,headRefName` prints with a release PR open
/// among a feature branch and a dependency bump.
const OPEN_WITH_RELEASE: &str = r#"[
  {"headRefName":"nick/9/sparse-target-report","number":12},
  {"headRefName":"release-plz-2026-10-14T09-30-00Z","number":13},
  {"headRefName":"dependabot/cargo/serde-1.0.228","number":11}
]"#;

fn assert_listed_open_pulls(armed: &Armed) {
    let list = armed
        .calls
        .iter()
        .find(|call| call[1..].starts_with(&["pr".to_owned(), "list".to_owned()]))
        .expect("the step lists the open pull requests");
    assert_eq!(
        list[0],
        format!("GITHUB_TOKEN={TOKEN}"),
        "gh is handed the step's token"
    );
    let args = list[1..].join(" ");
    assert!(
        args.contains(&format!("--repo {REPOSITORY}")) && args.contains("--state open"),
        "the step lists this repository's open pull requests: gh {args}"
    );
}

#[test]
fn the_arming_step_arms_auto_merge_on_the_open_release_pull_request() {
    let armed = run_arming_step(OPEN_WITH_RELEASE, 0);
    assert!(
        armed.status.success(),
        "the step succeeds: {}\n{}",
        armed.stdout,
        armed.stderr
    );
    assert_listed_open_pulls(&armed);
    let merges = armed.merges();
    assert_eq!(
        merges.len(),
        1,
        "exactly one gh pr merge: {:?}",
        armed.calls
    );
    assert_eq!(
        merges[0],
        [
            "pr",
            "merge",
            "13",
            "--repo",
            REPOSITORY,
            "--auto",
            "--squash",
            "--delete-branch"
        ],
        "auto-merge is armed on the release-plz-* pull request alone"
    );
    let merge_call = armed
        .calls
        .iter()
        .find(|call| call[1..] == *merges[0])
        .expect("the merge call");
    assert_eq!(merge_call[0], format!("GITHUB_TOKEN={TOKEN}"));
    assert!(
        armed
            .stdout
            .contains("Enabling auto-merge on release PR #13"),
        "{}",
        armed.stdout
    );
}

#[test]
fn the_arming_step_merges_nothing_when_no_release_pull_request_is_open() {
    let open_without_release = r#"[
  {"headRefName":"nick/9/sparse-target-report","number":12},
  {"headRefName":"dependabot/cargo/serde-1.0.228","number":11}
]"#;
    for pulls in [open_without_release, "[]"] {
        let armed = run_arming_step(pulls, 0);
        assert!(
            armed.status.success(),
            "with no release PR open the step still succeeds: {}\n{}",
            armed.stdout,
            armed.stderr
        );
        assert_listed_open_pulls(&armed);
        assert!(
            armed.merges().is_empty(),
            "no gh pr merge without an open release PR: {:?}",
            armed.calls
        );
        assert!(
            armed.stdout.contains("No open release PR to auto-merge."),
            "{}",
            armed.stdout
        );
    }
}

/// `gh pr merge --auto` refuses when auto-merge is already armed or the PR is already
/// merging; the step reports that as a warning and leaves the release job green.
#[test]
fn a_refused_arming_warns_and_leaves_the_job_green() {
    let armed = run_arming_step(OPEN_WITH_RELEASE, 1);
    assert!(
        armed.status.success(),
        "a refused gh pr merge must not fail the job: {}\n{}",
        armed.stdout,
        armed.stderr
    );
    assert_eq!(armed.merges().len(), 1, "{:?}", armed.calls);
    assert!(
        armed.stdout.lines().any(|line| line
            == "::warning::could not enable auto-merge on #13 (already enabled, or already merging)"),
        "the step must warn that arming failed: {}",
        armed.stdout
    );
}
