//! Symlink and permission safety through the subprocess boundary.
use super::{binary, fixture::*};
use serde_json::Value;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn pass(targets: &[&Path]) -> Value {
    let out = binary()
        .args(["--keep-within", "2d", "--json"])
        .args(targets)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&out).unwrap()
}

#[test]
fn every_symlink_traversal_path_preserves_its_outside_referent() {
    let temp = tempdir();
    let outside = temp.path().join("outside");
    basic(&outside);
    let target = temp.path().join("target");
    basic(&target);
    let alias = temp.path().join("alias");
    symlink(&outside, &alias).unwrap();
    symlink(outside.join("debug"), target.join("escaped-profile")).unwrap();
    for container in ["deps", ".fingerprint", "build", "incremental"] {
        let profile = target.join(format!("symlink-{container}"));
        fs::create_dir_all(profile.join(".fingerprint")).unwrap();
        if container == ".fingerprint" {
            fs::remove_dir(profile.join(container)).unwrap();
        }
        symlink(
            outside.join("debug").join(container),
            profile.join(container),
        )
        .unwrap();
    }
    let profile = target.join("debug");
    let build_link = profile.join(format!("build/alpha-{A}"));
    fs::remove_dir_all(&build_link).unwrap();
    symlink(
        outside.join("debug/build/alpha-0000000000000003"),
        &build_link,
    )
    .unwrap();
    let inc_link = profile.join(format!("incremental/linkunit-{A}"));
    symlink(
        outside.join("debug/incremental/alpha-0000000000000003"),
        &inc_link,
    )
    .unwrap();
    build(&profile, "linkunit", B, MIDDLE);
    for container in ["build", "incremental"] {
        let nested = profile.join(format!("{container}/beta-{A}/one/two/escape"));
        symlink(outside.join("debug"), &nested).unwrap();
        // Creating the symlink refreshes the parent's mtime; backdate ordinary
        // directories again, leaving the recent outside referent and link alone.
        age_tree(&profile.join(format!("{container}/beta-{A}")), OLD);
    }
    refresh_atime(&target);
    let outside_before = inventory(&outside);
    // Refused links stay in place: the profiles holding them, and the escaped profile link.
    let refused_before: Vec<_> = ["deps", ".fingerprint", "build", "incremental"]
        .iter()
        .map(|container| target.join(format!("symlink-{container}")))
        .chain([target.join("escaped-profile")])
        .map(|path| {
            let value = inventory(&path);
            (path, value)
        })
        .collect();
    let ancestor_alias = alias.join("debug");
    let report = pass(&[&alias, &ancestor_alias, &target]);
    assert_eq!(inventory(&outside), outside_before);
    for (path, value) in refused_before {
        assert_eq!(inventory(&path), value);
    }
    expect_removed(&profile, "alpha", A);
    expect_removed(&profile, "beta", A);
    assert!(fs::symlink_metadata(&build_link).is_err());
    assert_eq!(report["targets"][2]["builds_removed"], 2);
    let first = &report["targets"][0];
    assert_eq!(first["skipped"][0]["path"], alias.to_str().unwrap());
    assert!(
        first["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("refused")
    );
    assert!(
        report["targets"][1]["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("refused")
    );
    let skipped = report["targets"][2]["skipped"].as_array().unwrap();
    for expected in [
        target.join("escaped-profile"),
        target.join("symlink-deps/deps"),
        target.join("symlink-.fingerprint/.fingerprint"),
        target.join("symlink-build/build"),
        target.join("symlink-incremental/incremental"),
    ] {
        assert!(
            skipped
                .iter()
                .any(|s| s["path"] == expected.to_str().unwrap())
        );
    }
}

fn constrained_binary() -> assert_cmd::Command {
    let built = binary();
    let path = PathBuf::from(built.get_program());
    let mut command = if std::process::Command::new("id")
        .arg("-u")
        .output()
        .unwrap()
        .stdout
        == b"0\n"
    {
        let mut command = assert_cmd::Command::new("setpriv");
        command
            .args(["--reuid=65534", "--regid=65534", "--clear-groups"])
            .arg(path);
        command
    } else {
        built
    };
    command.env_remove("ONEBUDGETSPEC_RESULT");
    command
}

fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn writable(path: &Path) {
    if path.is_dir() {
        chmod(path, 0o777);
        for entry in fs::read_dir(path).unwrap() {
            writable(&entry.unwrap().path());
        }
    }
}

#[test]
fn actual_removal_refusal_is_reported_and_other_builds_continue() {
    let temp = tempdir();
    chmod(temp.path(), 0o777);
    let target = temp.path().join("target");
    basic(&target);
    writable(&target);
    let denied = target.join(format!("debug/build/alpha-{A}/one/two"));
    chmod(&denied, 0o555);
    let output = constrained_binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&target)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    chmod(&denied, 0o777);
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(report["targets"][0]["builds_removed"], 1);
    expect_removed(&target.join("debug"), "beta", A);
    assert!(denied.join("leaf").exists());
    let skipped = report["targets"][0]["skipped"].as_array().unwrap();
    assert!(
        skipped
            .iter()
            .any(|s| s["path"] == denied.join("leaf").to_str().unwrap()
                && s["reason"].as_str().unwrap().contains("Permission denied"))
    );
}

#[test]
fn unreadable_target_exits_one_without_modifying_it() {
    let temp = tempdir();
    chmod(temp.path(), 0o777);
    let target = temp.path().join("target");
    basic(&target);
    let before = inventory(&target);
    chmod(&target, 0o000);
    let out = constrained_binary()
        .args(["--keep-within", "2d"])
        .arg(&target)
        .assert()
        .code(1)
        .get_output()
        .stderr
        .clone();
    chmod(&target, 0o755);
    assert!(
        String::from_utf8(out)
            .unwrap()
            .contains(target.to_str().unwrap())
    );
    assert_eq!(inventory(&target), before);
}

#[test]
fn human_summary_sizes_dry_run_and_skipped_lines_match_filesystem() {
    let temp = tempdir();
    let target = temp.path().join("target");
    basic(&target);
    let before = inventory(&target);
    let profile = target.join("debug");
    let removed = du(&[paths(&profile, "alpha", A), paths(&profile, "beta", A)].concat());
    let retained = du(&[("alpha", B), ("alpha", C), ("beta", B)]
        .iter()
        .flat_map(|(u, h)| paths(&profile, u, h))
        .collect::<Vec<_>>());
    let absent = temp.path().join("absent");
    let out = binary()
        .args(["--keep-within", "2d", "--dry-run"])
        .arg(&target)
        .arg(&absent)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let output = String::from_utf8(out).unwrap();
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(lines.len(), 3);
    assert_summary(lines[0], &target, "would remove", (2, 1, removed, retained));
    assert_summary(lines[1], &absent, "would remove", (0, 0, 0, 0));
    assert_eq!(
        lines[2],
        format!("  skipped {}: target directory is absent", absent.display())
    );
    assert_eq!(inventory(&target), before);
    let out = binary()
        .args(["--keep-within", "2d"])
        .arg(&target)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    assert_summary(
        String::from_utf8(out).unwrap().trim(),
        &target,
        "removed",
        (2, 1, removed, retained),
    );
    expect_removed(&profile, "alpha", A);
    expect_removed(&profile, "beta", A);
}

fn assert_summary(line: &str, path: &Path, verb: &str, expected: (u64, u64, u64, u64)) {
    let prefix = format!("cargo-prune-superseded: {}: {verb} ", path.display());
    let rest = line.strip_prefix(&prefix).unwrap();
    let (builds, rest) = rest.split_once(" superseded build(s) across ").unwrap();
    let (profiles, rest) = rest.split_once(" profile(s), freed ").unwrap();
    let (freed, kept) = rest.split_once(" B, kept ").unwrap();
    let kept = kept.strip_suffix(" B").unwrap();
    assert_eq!(
        (
            builds.parse::<u64>().unwrap(),
            profiles.parse::<u64>().unwrap(),
            freed.parse::<u64>().unwrap(),
            kept.parse::<u64>().unwrap()
        ),
        expected
    );
}

#[test]
fn unreadable_build_or_container_protects_uncertain_output_and_other_profiles_continue() {
    // Unlistable build/ entries, an unlistable build/ container, and a listable build/
    // whose entries cannot be inspected (no search permission).
    for (denied, mode, skipped) in [
        (
            format!("build/alpha-{A}/one/two"),
            0o000,
            format!("build/alpha-{A}/one/two"),
        ),
        ("build".to_owned(), 0o000, "build".to_owned()),
        ("build".to_owned(), 0o444, format!("build/alpha-{A}")),
    ] {
        let temp = tempdir();
        chmod(temp.path(), 0o777);
        let target = temp.path().join("target");
        basic(&target);
        build(&target.join("release"), "alpha", A, OLD);
        build(&target.join("release"), "alpha", B, MIDDLE);
        writable(&target);
        let profile = target.join("debug");
        let before: Vec<_> = [A, B, C]
            .iter()
            .flat_map(|hash| paths(&profile, "alpha", hash).into_iter().take(4))
            .map(|path| {
                let value = inventory(&path);
                (path, value)
            })
            .collect();
        let denied = profile.join(denied);
        chmod(&denied, mode);
        let output = constrained_binary()
            .args(["--keep-within", "2d", "--json"])
            .arg(&target)
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        chmod(&denied, 0o777);
        let report: Value = serde_json::from_slice(&output).unwrap();
        assert!(
            report["targets"][0]["skipped"]
                .as_array()
                .unwrap()
                .iter()
                .any(|s| s["path"] == profile.join(&skipped).to_str().unwrap())
        );
        for (path, value) in before {
            assert_eq!(inventory(&path), value);
        }
        expect_removed(&target.join("release"), "alpha", A);
    }
}

#[test]
fn human_real_refusal_has_one_line_per_skipped_entry_and_measured_sizes() {
    let temp = tempdir();
    chmod(temp.path(), 0o777);
    let target = temp.path().join("target");
    basic(&target);
    writable(&target);
    let profile = target.join("debug");
    let denied = profile.join(format!("build/alpha-{A}/one/two"));
    chmod(&denied, 0o555);
    let before_du = du(std::slice::from_ref(&target));
    let output = constrained_binary()
        .args(["--keep-within", "2d"])
        .arg(&target)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    chmod(&denied, 0o777);
    let freed = before_du - du(std::slice::from_ref(&target));
    let retained_paths: Vec<_> = [("alpha", A), ("alpha", B), ("alpha", C), ("beta", B)]
        .iter()
        .flat_map(|(u, h)| paths(&profile, u, h))
        .filter(|p| p.exists())
        .collect();
    let retained = du(&retained_paths);
    let text = String::from_utf8(output).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_summary(lines[0], &target, "removed", (1, 1, freed, retained));
    let expected = [
        denied.join("leaf"),
        denied.clone(),
        denied.parent().unwrap().to_owned(),
        profile.join(format!("build/alpha-{A}")),
    ];
    assert_eq!(lines.len(), expected.len() + 1);
    for (line, path) in lines[1..].iter().zip(expected) {
        let reason = line
            .strip_prefix(&format!("  skipped {}: ", path.display()))
            .unwrap();
        assert_ne!(reason, "");
    }
    expect_removed(&profile, "beta", A);
}

#[test]
fn unreadable_discovery_directory_is_skipped_and_readable_profile_continues() {
    // An unlistable directory, and a listable one whose children cannot be inspected.
    for (mode, skipped) in [(0o000, None), (0o444, Some("debug"))] {
        unreadable_discovery_directory(mode, skipped);
    }
}

fn unreadable_discovery_directory(mode: u32, skipped: Option<&str>) {
    let temp = tempdir();
    chmod(temp.path(), 0o777);
    let target = temp.path().join("target");
    basic(&target);
    let hidden = target.join("hidden");
    basic(&hidden);
    let before = inventory(&hidden);
    chmod(&hidden, mode);
    let output = constrained_binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&target)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    chmod(&hidden, 0o755);
    let report: Value = serde_json::from_slice(&output).unwrap();
    assert_eq!(
        report["targets"][0]["skipped"][0]["path"],
        skipped
            .map_or_else(|| hidden.clone(), |child| hidden.join(child))
            .to_str()
            .unwrap()
    );
    assert_eq!(report["targets"][0]["skipped"].as_array().unwrap().len(), 1);
    assert_eq!(inventory(&hidden), before);
    expect_removed(&target.join("debug"), "alpha", A);
}

#[test]
fn failed_target_preserves_valid_json_and_another_target_completes() {
    let temp = tempdir();
    chmod(temp.path(), 0o777);
    let bad = temp.path().join("unreadable");
    basic(&bad);
    let good = temp.path().join("good");
    basic(&good);
    writable(&good);
    let before = inventory(&bad);
    chmod(&bad, 0o000);
    let output = constrained_binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&bad)
        .arg(&good)
        .assert()
        .code(1)
        .get_output()
        .clone();
    chmod(&bad, 0o755);
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains(bad.to_str().unwrap())
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["targets"].as_array().unwrap().len(), 2);
    assert_eq!(report["targets"][1]["path"], good.to_str().unwrap());
    assert_eq!(report["targets"][0]["path"], bad.to_str().unwrap());
    assert_eq!(
        report["targets"][0]["skipped"][0]["path"],
        bad.to_str().unwrap()
    );
    assert_eq!(report["targets"][0]["builds_removed"], 0);
    assert_eq!(report["targets"][1]["builds_removed"], 2);
    expect_removed(&good.join("debug"), "alpha", A);
    assert_eq!(inventory(&bad), before);
}

#[test]
#[cfg_attr(
    target_os = "macos",
    ignore = "APFS refuses non-UTF-8 file names (EILSEQ), so no such target can exist there"
)]
fn non_utf8_target_paths_prune_and_produce_valid_json() {
    use std::os::unix::ffi::OsStringExt;
    let temp = tempdir();
    let target = temp
        .path()
        .join(std::ffi::OsString::from_vec(b"target-\xff".to_vec()));
    basic(&target);
    let report = pass(&[&target]);
    assert_eq!(
        report["targets"][0]["path"],
        target.to_string_lossy().as_ref()
    );
    assert_eq!(report["targets"][0]["builds_removed"], 2);
    expect_removed(&target.join("debug"), "alpha", A);
    let absent = target.join(std::ffi::OsString::from_vec(b"absent-\xff".to_vec()));
    let report = pass(&[&absent]);
    assert_eq!(
        report["targets"][0]["skipped"][0]["path"],
        absent.to_string_lossy().as_ref()
    );
}
