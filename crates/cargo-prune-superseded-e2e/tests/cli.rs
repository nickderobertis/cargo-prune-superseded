//! The `cargo-prune-superseded` binary, spawned as a subprocess the way a user runs it,
//! asserting exit status, stdout and stderr.

use std::path::PathBuf;

#[path = "support/binary.rs"]
mod binary_driver;
use binary_driver::binary;
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
fn no_arguments_requires_a_window() {
    binary()
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--keep-within"));
}

use prune_fixtures as fixture;

#[path = "support/pruning.rs"]
mod pruning;

#[cfg(unix)]
#[path = "support/safety.rs"]
mod safety;
