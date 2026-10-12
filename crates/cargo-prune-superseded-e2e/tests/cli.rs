//! The `cargo-prune-superseded` binary, spawned as a subprocess the way a user runs it,
//! asserting exit status, stdout and stderr.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use assert_cmd::Command;
use predicates::prelude::*;

/// The version the binary must report: the one the crate's manifest declares, read
/// from the manifest itself rather than restated here.
fn manifest_version() -> String {
    let manifest = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../cargo-prune-superseded/Cargo.toml"),
    )
    .expect("the binary crate's manifest is readable");
    let workspace =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.toml"))
            .expect("the workspace manifest is readable");
    let source = if manifest.contains("version.workspace = true") {
        workspace
    } else {
        manifest
    };
    source
        .lines()
        .find_map(|line| {
            line.strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .expect("a version line in the manifest")
        .to_owned()
}

/// The built binary, in the target directory this test executable was built into
/// (`<target>/<profile>/deps/<test>`), where cargo puts a workspace member's binaries.
/// `cargo test` does not build another package's binary, so it is built here, once per
/// run, into the same target directory and profile with the environment this run
/// inherited (under `cargo llvm-cov` that is the instrumented build), so a journey never
/// drives a stale binary. It is a no-op when the binary is already up to date.
fn binary() -> Command {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let path = BUILT.get_or_init(|| {
        let exe = std::env::current_exe().expect("the test executable's path");
        let profile_dir = exe
            .parent()
            .and_then(Path::parent)
            .expect("the test executable sits in <target>/<profile>/deps");
        let path = profile_dir.join(format!(
            "cargo-prune-superseded{}",
            std::env::consts::EXE_SUFFIX
        ));
        {
            let target_dir = profile_dir.parent().expect("<target>/<profile>");
            let mut build = std::process::Command::new(env!("CARGO"));
            build
                .args([
                    "build",
                    "--quiet",
                    "--locked",
                    "-p",
                    "cargo-prune-superseded",
                ])
                .arg("--target-dir")
                .arg(target_dir);
            if profile_dir
                .file_name()
                .is_some_and(|name| name == "release")
            {
                build.arg("--release");
            }
            let status = build.status().expect("cargo spawns");
            assert!(
                status.success() && path.is_file(),
                "building {} failed ({status}); run `cargo build -p cargo-prune-superseded` \
                 to see why, then re-run",
                path.display()
            );
        }
        path
    });
    Command::new(path)
}

#[test]
fn version_prints_the_crate_version() {
    binary()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("cargo-prune-superseded {}\n", manifest_version()))
        .stderr("");
}

#[test]
fn help_prints_usage_naming_the_binary() {
    binary()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: cargo-prune-superseded"))
        .stderr("");
}

#[test]
fn an_unknown_flag_fails_with_an_error_on_stderr() {
    binary()
        .arg("--no-such-flag")
        .assert()
        .failure()
        .code(2)
        .stdout("")
        .stderr(predicate::str::contains(
            "unexpected argument '--no-such-flag'",
        ))
        .stderr(predicate::str::contains("--help"));
}

#[test]
fn no_arguments_succeeds_and_leaves_the_directory_untouched() {
    let project = tempfile::tempdir().expect("a project directory");
    let artifact = project.path().join("target/debug/deps/libold-0123.rlib");
    std::fs::create_dir_all(artifact.parent().expect("a parent")).expect("a target tree");
    std::fs::write(&artifact, b"stale").expect("a build artifact");
    binary()
        .current_dir(project.path())
        .assert()
        .success()
        .stdout("")
        .stderr("");
    assert_eq!(
        std::fs::read(&artifact).expect("the artifact is still there"),
        b"stale"
    );
}
