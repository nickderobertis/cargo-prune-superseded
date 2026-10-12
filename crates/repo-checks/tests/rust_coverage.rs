//! `tools/rust-coverage.sh`, run from a scratch copy of the repository layout (so its
//! run markers never touch this checkout's real coverage run) with a recording `cargo`
//! first on PATH. cargo-llvm-cov is the tool the script orchestrates; its own
//! instrumentation is proven by the gate's real coverage run, so here each journey asserts
//! what the script asked of it and the verdict it reached.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

/// A scratch checkout holding a copy of the script, and a recording `cargo`.
struct Checkout {
    root: tempfile::TempDir,
    bin: tempfile::TempDir,
}

impl Checkout {
    /// `failing` names the cargo subcommand words (e.g. `"llvm-cov report"`) that exit 1.
    fn new(failing: &[&str]) -> Self {
        Self::measuring(failing, 10)
    }

    /// As `new`, with `llvm-cov report --summary-only --json` answering that the runs
    /// measured `lines` production lines.
    fn measuring(failing: &[&str], lines: u32) -> Self {
        Self::summarising(
            failing,
            &format!(
                r#"{{"data":[{{"totals":{{"lines":{{"count":{lines},"covered":{lines}}}}}}}]}}"#
            ),
        )
    }

    /// As `new`, with `llvm-cov report --summary-only --json` printing `summary`.
    fn summarising(failing: &[&str], summary: &str) -> Self {
        let root = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir(root.path().join("tools")).expect("tools/");
        let script = root.path().join("tools/rust-coverage.sh");
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/rust-coverage.sh"),
            &script,
        )
        .expect("the script");
        std::fs::write(root.path().join("summary.json"), summary).expect("a summary");
        let bin = tempfile::tempdir().expect("a PATH directory");
        let failures: Vec<String> = failing
            .iter()
            .map(|words| {
                format!(
                    "case \"$*\" in \"{words}\"*) echo \"cargo {words}: failed\" >&2; exit 1;; esac"
                )
            })
            .collect();
        let failures = failures.join("\n");
        // llmlint: ignore-block[e2e_not_mocked] cargo-llvm-cov is the external tool this script orchestrates, and its instrumentation is exercised for real by the gate's own coverage run; the recording cargo stands only for that boundary so each of the script's decisions can be asserted.
        let cargo = bin.path().join("cargo");
        std::fs::write(
            &cargo,
            format!(
                "#!/usr/bin/env bash\necho \"$*\" >>{log:?}\n{failures}\n\
                 case \"$*\" in *--json*) cat {summary_file:?};; esac\nexit 0\n",
                log = root.path().join("cargo-calls").display().to_string(),
                summary_file = root.path().join("summary.json").display().to_string(),
            ),
        )
        .expect("the cargo stub");
        // llmlint: ignore-end[e2e_not_mocked]
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).expect("+x");
        Self { root, bin }
    }

    fn run(&self, request: &str) -> Output {
        let mut attempts = 0;
        loop {
            let result =
                std::process::Command::new(self.root.path().join("tools/rust-coverage.sh"))
                    .arg(request)
                    .env(
                        "PATH",
                        format!("{}:/usr/bin:/bin", self.bin.path().display()),
                    )
                    .output();
            match result {
                // ETXTBSY: another test thread forked while a stub was being written.
                Err(error) if error.raw_os_error() == Some(26) && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => return result.expect("the script runs"),
            }
        }
    }

    /// The cargo calls past the two tool probes every run makes first.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.path().join("cargo-calls"))
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.ends_with("--version"))
            .map(str::to_owned)
            .collect()
    }

    fn marker(&self, package: &str) -> PathBuf {
        self.root.path().join("target/coverage-runs").join(package)
    }
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

#[test]
fn a_crate_run_measures_that_package_and_records_it() {
    let checkout = Checkout::new(&[]);
    let output = checkout.run("some-crate");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        checkout.calls(),
        ["llvm-cov --no-report nextest --locked --no-tests=pass --package some-crate"]
    );
    assert!(checkout.marker("some-crate").is_file());
}

#[test]
fn failing_tests_stop_the_run_and_record_nothing() {
    let checkout = Checkout::new(&["llvm-cov --no-report nextest"]);
    let output = checkout.run("some-crate");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("some-crate's tests failed under instrumentation (above)"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(!checkout.marker("some-crate").exists());
}

#[test]
fn clearing_empties_the_profiles_and_the_run_markers() {
    let checkout = Checkout::new(&[]);
    checkout.run("some-crate");
    let output = checkout.run("--clear");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert!(!checkout.marker("some-crate").exists());
    assert_eq!(
        checkout.calls().last().map(String::as_str),
        Some("llvm-cov clean --workspace")
    );
}

#[test]
fn the_report_holds_the_floor_only_over_runs_since_the_clear() {
    let checkout = Checkout::new(&[]);
    let skipped = checkout.run("--report");
    assert_eq!(skipped.status.code(), Some(0));
    assert_eq!(checkout.calls(), Vec::<String>::new(), "no run, no report");
    checkout.run("some-crate");
    let output = checkout.run("--report");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        checkout.calls().last().map(String::as_str),
        Some("llvm-cov report --summary-only --show-missing-lines --fail-under-lines 95")
    );
}

#[test]
fn a_summary_cargo_llvm_cov_cannot_produce_is_reported() {
    let checkout = Checkout::new(&["llvm-cov report --summary-only --json"]);
    checkout.run("some-crate");
    let output = checkout.run("--report");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output)
            .contains("cargo llvm-cov could not summarise the runs (above); run 'just coverage'"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn runs_that_measured_no_production_line_hold_no_floor() {
    // A change reaching only test-only crates: their suites ran, but measured nothing.
    let checkout = Checkout::measuring(&["llvm-cov report --summary-only --show-missing-lines"], 0);
    checkout.run("some-e2e-crate");
    let output = checkout.run("--report");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        checkout.calls().last().map(String::as_str),
        Some("llvm-cov report --summary-only --json"),
        "the floor is not applied to zero measured lines"
    );
}

#[test]
fn a_report_below_the_floor_fails_with_what_to_do() {
    let checkout = Checkout::new(&["llvm-cov report --summary-only --show-missing-lines"]);
    checkout.run("some-crate");
    let output = checkout.run("--report");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("below 95% lines over every crate's run (above)"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_missing_coverage_tool_is_named_with_how_to_install_it() {
    let checkout = Checkout::new(&["llvm-cov --version"]);
    let output = checkout.run("--report");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("'cargo llvm-cov' did not run (above); install it with 'cargo install --locked cargo-llvm-cov'"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn an_argument_that_is_not_a_package_name_is_a_usage_error() {
    let checkout = Checkout::new(&[]);
    for request in ["bad name", "--unknown"] {
        let output = checkout.run(request);
        assert_eq!(output.status.code(), Some(64), "request: {request}");
        assert!(stderr(&output).contains("pass a workspace member's package name"));
    }
    assert_eq!(checkout.calls(), Vec::<String>::new());
}

#[test]
fn anything_but_exactly_one_argument_is_a_usage_error() {
    let checkout = Checkout::new(&[]);
    for args in [&[][..], &["--clear", "extra"][..]] {
        let output =
            std::process::Command::new(checkout.root.path().join("tools/rust-coverage.sh"))
                .args(args)
                .env(
                    "PATH",
                    format!("{}:/usr/bin:/bin", checkout.bin.path().display()),
                )
                .output()
                .expect("the script runs");
        assert_eq!(output.status.code(), Some(64), "args: {args:?}");
        assert!(stderr(&output).contains("takes exactly one argument"));
    }
    assert_eq!(checkout.calls(), Vec::<String>::new());
}

#[test]
fn a_summary_without_line_totals_is_refused_with_what_to_do() {
    let checkout = Checkout::summarising(&[], r#"{"data":[{"files":[]}]}"#);
    checkout.run("some-crate");
    let output = checkout.run("--report");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("summary has no totals.lines.count"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_failed_clear_keeps_the_markers_and_a_later_clear_recovers() {
    let failing = Checkout::new(&["llvm-cov clean"]);
    failing.run("some-crate");
    let output = failing.run("--clear");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("could not clear cargo-llvm-cov's profiles"),
        "stderr: {}",
        stderr(&output)
    );
    assert!(
        failing.marker("some-crate").is_file(),
        "a failed clear leaves the record of runs"
    );
}

#[test]
fn a_run_marker_that_cannot_be_written_is_reported_and_recovers() {
    let checkout = Checkout::new(&[]);
    let runs = checkout.root.path().join("target/coverage-runs");
    std::fs::create_dir_all(runs.parent().expect("target/")).expect("target/");
    std::fs::write(&runs, "not a directory").expect("a file where the marker dir goes");
    let output = checkout.run("some-crate");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("could not record some-crate's run"),
        "stderr: {}",
        stderr(&output)
    );
    std::fs::remove_file(&runs).expect("repair target/");
    let retried = checkout.run("some-crate");
    assert_eq!(
        retried.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&retried)
    );
    assert!(checkout.marker("some-crate").is_file());
}
