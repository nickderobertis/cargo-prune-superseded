//! `scripts/msrv.sh`, run from a scratch checkout whose `Cargo.toml` each journey writes,
//! with recording `rustup` and `cargo` first on PATH (installing a toolchain and building
//! under it are what the gate's real `just msrv` run proves): each journey asserts which
//! minimum the script read from the manifest and what it asked of them.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

fn run(manifest: &str, tools: &[&str], failing: Option<&str>) -> (Output, Vec<String>) {
    let root = tempfile::tempdir().expect("a checkout");
    std::fs::create_dir(root.path().join("scripts")).expect("scripts/");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/msrv.sh"),
        root.path().join("scripts/msrv.sh"),
    )
    .expect("the script");
    std::fs::write(root.path().join("Cargo.toml"), manifest).expect("a manifest");
    let bin = tempfile::tempdir().expect("a PATH directory");
    let log = root.path().join("calls");
    for tool in tools {
        let failure = failing.map_or(String::new(), |words| {
            format!("case \"$*\" in *\"{words}\"*) echo '{tool}: failed' >&2; exit 1;; esac\n")
        });
        let path = bin.path().join(tool);
        // llmlint: ignore-block[e2e_not_mocked] rustup and cargo download and build a whole toolchain, which this offline tier cannot; the gate's real `just msrv` run exercises them, and the recording stubs stand only for that boundary.
        std::fs::write(
            &path,
            format!(
                "#!/usr/bin/env bash\necho \"{tool} $*\" >>{log:?}\n{failure}exit 0\n",
                log = log.display().to_string()
            ),
        )
        .expect("a stub");
        // llmlint: ignore-end[e2e_not_mocked]
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("+x");
    }
    let mut attempts = 0;
    let output = loop {
        let result = std::process::Command::new(root.path().join("scripts/msrv.sh"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.path().display()))
            .output();
        match result {
            // ETXTBSY: another test thread forked while a stub was being written.
            Err(error) if error.raw_os_error() == Some(26) && attempts < 100 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => break result.expect("the script runs"),
        }
    };
    let calls = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    (output, calls)
}

const MANIFEST: &str = "[workspace]\nmembers = []\n\n[workspace.package]\nversion = \"0.1.0\"\nrust-version = \"1.89\"\n\n[workspace.dependencies]\n";

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

#[test]
fn builds_under_the_declared_minimum_quietly() {
    let (output, calls) = run(MANIFEST, &["rustup", "cargo"], None);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stderr(&output), "");
    assert_eq!(
        calls,
        [
            "rustup toolchain install 1.89 --profile minimal --no-self-update",
            "cargo +1.89 check --locked --all-targets --all-features",
        ]
    );
}

#[test]
fn a_manifest_without_exactly_one_workspace_minimum_is_refused() {
    for (manifest, reason) in [
        (
            "[workspace.package]\nversion = \"0.1.0\"\n\n[package]\nrust-version = \"1.70\"\n",
            "must declare exactly one rust-version (found: none)",
        ),
        (
            "[workspace.package]\nrust-version = \"1.89\"\nrust-version = \"1.90\"\n",
            "must declare exactly one rust-version",
        ),
        (
            "[workspace.package]\nrust-version = \"1.89\"\nrust-version=latest\n",
            "must declare exactly one rust-version",
        ),
        (
            "[workspace.package]\nrust-version = \"latest\"\n",
            "is not rust-version = \"X.Y\" or \"X.Y.Z\"",
        ),
    ] {
        let (output, calls) = run(manifest, &["rustup", "cargo"], None);
        assert_eq!(output.status.code(), Some(1), "manifest: {manifest}");
        assert!(
            stderr(&output).contains(reason),
            "stderr: {}",
            stderr(&output)
        );
        assert_eq!(calls, Vec::<String>::new());
    }
}

#[test]
fn a_host_without_rustup_is_told_where_to_get_it() {
    let (output, calls) = run(MANIFEST, &["cargo"], None);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("rustup is not installed, so Rust 1.89 cannot be selected"),
        "stderr: {}",
        stderr(&output)
    );
    assert_eq!(calls, Vec::<String>::new());
}

#[test]
fn code_that_does_not_build_under_the_minimum_fails_with_what_to_do() {
    let (output, _) = run(MANIFEST, &["rustup", "cargo"], Some("check"));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("cargo: failed")
            && stderr(&output).contains(
                "failed (above) under Rust 1.89; fix it, or raise rust-version deliberately"
            ),
        "stderr: {}",
        stderr(&output)
    );
}
