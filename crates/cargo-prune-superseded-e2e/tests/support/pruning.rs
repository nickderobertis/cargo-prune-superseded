//! Real binary journeys for selection, safety, failures and reporting.
use super::{binary, fixture::*};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn json(targets: &[&Path], extra: &[&str]) -> Value {
    let output = binary()
        .args(["--keep-within", "2d", "--json"])
        .args(extra)
        .args(targets)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    serde_json::from_slice(&output).unwrap()
}

fn untouched(target: &Path) -> Vec<PathBuf> {
    let mut paths = vec![
        target.join("doc/docs/leaf"),
        target.join("tmp/cache/leaf"),
        target.join("odd/leaf"),
        target.join("unknown"),
        target.join("debug/root"),
    ];
    for container in ["deps", ".fingerprint", "build", "incremental"] {
        paths.push(target.join(format!("debug/{container}/unrecognized/leaf")));
        paths.push(target.join(format!("debug/{container}/unrecognized-file")));
    }
    for path in &paths {
        write(path);
    }
    paths
}

#[test]
fn modification_time_selection_and_allocated_bytes_with_external_hardlink() {
    let temp = tempdir();
    let target = temp.path().join("target");
    basic(&target);
    let ignored = untouched(&target);
    let ignored_before: Vec<_> = ignored.iter().map(|p| inventory(p)).collect();
    let profile = target.join("debug");
    let kept_units = [("alpha", B), ("alpha", C), ("beta", B)];
    let before_kept = kept(&profile, &kept_units);
    let links = temp.path().join("links");
    fs::create_dir(&links).unwrap();
    let linked = paths(&profile, "alpha", A)[0].clone();
    fs::hard_link(&linked, links.join("artifact")).unwrap();
    let removed_paths: Vec<_> = [paths(&profile, "alpha", A), paths(&profile, "beta", A)].concat();
    let removed_du = du(&removed_paths);
    let linked_blocks = du(&[linked]);
    let retained_paths: Vec<_> = kept_units
        .iter()
        .flat_map(|(unit, hash)| paths(&profile, unit, hash))
        .collect();
    let retained_du = du(&retained_paths);
    let before_du = du(&[target.clone(), links.clone()]);
    refresh_atime(&target);
    let dry_before = inventory(&target);
    let dry = json(&[&target], &["--dry-run"]);
    assert_eq!(
        dry["targets"][0]["bytes_freed"].as_u64().unwrap(),
        removed_du - linked_blocks
    );
    assert_eq!(
        dry["targets"][0]["bytes_retained"].as_u64().unwrap(),
        retained_du
    );
    assert_eq!(inventory(&target), dry_before);
    let report = json(&[&target], &[]);
    expect_removed(&profile, "alpha", A);
    expect_removed(&profile, "beta", A);
    assert_eq!(kept(&profile, &kept_units), before_kept);
    assert_eq!(fs::read(links.join("artifact")).unwrap(), vec![b'x'; 8192]);
    for (path, before) in ignored.iter().zip(ignored_before) {
        assert_eq!(inventory(path), before);
    }
    let after_du = du(&[target.clone(), links]);
    let result = &report["targets"][0];
    assert_eq!(result["path"], target.to_str().unwrap());
    assert_eq!(result["profiles"], 1);
    assert_eq!(result["builds_removed"], 2);
    assert_eq!(
        result["bytes_freed"].as_u64().unwrap(),
        before_du - after_du
    );
    assert_eq!(before_du - after_du, removed_du - linked_blocks);
    assert_eq!(result["bytes_retained"].as_u64().unwrap(), retained_du);
    assert_eq!(result["skipped"], serde_json::json!([]));
}

#[test]
fn profiles_and_units_select_independently() {
    let temp = tempdir();
    let target = temp.path().join("target");
    for (i, name) in [
        "debug",
        "release",
        "aarch64-unknown-linux-gnu/debug",
        "llvm-cov-target/debug",
    ]
    .iter()
    .enumerate()
    {
        let profile = target.join(name);
        let (old, newest) = if i % 2 == 0 { (A, B) } else { (B, A) };
        build(
            &profile,
            "alpha",
            old,
            OLD + u64::try_from(i).unwrap() * 86400,
        );
        build(
            &profile,
            "alpha",
            newest,
            MIDDLE + u64::try_from(i).unwrap() * 86400,
        );
        build(&profile, "beta", old, OLD + 86400);
        build(&profile, "beta", newest, MIDDLE + 86400);
    }
    let before: Vec<_> = [
        "debug",
        "release",
        "aarch64-unknown-linux-gnu/debug",
        "llvm-cov-target/debug",
    ]
    .iter()
    .enumerate()
    .map(|(i, name)| {
        let newest = if i % 2 == 0 { B } else { A };
        kept(&target.join(name), &[("alpha", newest), ("beta", newest)])
    })
    .collect();
    let report = json(&[&target], &[]);
    assert_eq!(report["targets"][0]["profiles"], 4);
    assert_eq!(report["targets"][0]["builds_removed"], 8);
    for ((i, name), before) in [
        "debug",
        "release",
        "aarch64-unknown-linux-gnu/debug",
        "llvm-cov-target/debug",
    ]
    .iter()
    .enumerate()
    .zip(before)
    {
        let (old, newest) = if i % 2 == 0 { (A, B) } else { (B, A) };
        expect_removed(&target.join(name), "alpha", old);
        expect_removed(&target.join(name), "beta", old);
        assert_eq!(
            kept(&target.join(name), &[("alpha", newest), ("beta", newest)]),
            before
        );
    }
}

#[test]
fn deepest_file_and_directory_mtimes_keep_whole_builds() {
    for recent_directory in [false, true] {
        let temp = tempdir();
        let profile = temp.path().join("target/debug");
        build(&profile, "gamma", A, OLD);
        build(&profile, "gamma", B, RECENT);
        build(&profile, "expired", A, OLD);
        build(&profile, "expired", B, MIDDLE);
        for container in ["build", "incremental"] {
            let nested = profile.join(format!("{container}/gamma-{A}/one/two"));
            recent(
                &if recent_directory {
                    nested
                } else {
                    nested.join("leaf")
                },
                WINDOW,
            );
        }
        let before = kept(&profile, &[("gamma", A), ("gamma", B)]);
        json(&[&temp.path().join("target")], &[]);
        assert_eq!(kept(&profile, &[("gamma", A), ("gamma", B)]), before);
        expect_removed(&profile, "expired", A);
    }
}

#[test]
fn dry_run_default_relative_multiple_targets_and_subcommand_form() {
    let temp = tempdir();
    let target = temp.path().join("target");
    let other = temp.path().join("other");
    basic(&target);
    basic(&other);
    let before = inventory(&target);
    let other_before = inventory(&other);
    let report = json(&[&target], &["--dry-run"]);
    assert_eq!(report["targets"][0]["builds_removed"], 2);
    assert_eq!(inventory(&target), before);
    let default_kept = kept(
        &target.join("debug"),
        &[("alpha", B), ("alpha", C), ("beta", B)],
    );
    binary()
        .current_dir(temp.path())
        .args(["--keep-within", "2d"])
        .assert()
        .success();
    expect_removed(&target.join("debug"), "alpha", A);
    expect_removed(&target.join("debug"), "beta", A);
    assert_eq!(
        kept(
            &target.join("debug"),
            &[("alpha", B), ("alpha", C), ("beta", B)]
        ),
        default_kept
    );
    assert_eq!(inventory(&other), other_before);
    basic(&target);
    let out = binary()
        .current_dir(temp.path())
        .args([
            "prune-superseded",
            "--keep-within",
            "2d",
            "--json",
            "target",
            "other",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let report: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(report["targets"].as_array().unwrap().len(), 2);
    for (i, name) in ["target", "other"].iter().enumerate() {
        assert_eq!(report["targets"][i]["path"], *name);
        assert_eq!(report["targets"][i]["builds_removed"], 2);
        expect_removed(&temp.path().join(name).join("debug"), "alpha", A);
        expect_removed(&temp.path().join(name).join("debug"), "beta", A);
    }
}

#[test]
fn usage_span_grammar_and_real_retention_for_each_unit() {
    for invalid in [
        "0s",
        "-2d",
        "1.5h",
        "2w",
        "18446744073709551616s",
        "18446744073709551615d",
        "",
        "s",
    ] {
        let output = binary()
            .args(["--keep-within", invalid])
            .assert()
            .code(2)
            .get_output()
            .stderr
            .clone();
        assert!(String::from_utf8(output).unwrap().contains(invalid));
    }
    binary().arg("--keep-within").assert().code(2);
    for span in ["86400s", "1440m", "24h", "1d"] {
        let temp = tempdir();
        let profile = temp.path().join("debug");
        build(&profile, "alpha", A, OLD);
        build(&profile, "alpha", B, 3600);
        build(&profile, "alpha", C, 60);
        let before = kept(&profile, &[("alpha", B), ("alpha", C)]);
        binary()
            .args(["--keep-within", span])
            .arg(temp.path())
            .assert()
            .success();
        expect_removed(&profile, "alpha", A);
        assert_eq!(kept(&profile, &[("alpha", B), ("alpha", C)]), before);
    }
}

#[test]
fn absent_targets_and_budget_reporting() {
    let temp = tempdir();
    let absent = temp.path().join("absent");
    let report = json(&[&absent], &[]);
    assert_eq!(report["targets"][0]["profiles"], 0);
    assert_eq!(
        report["targets"][0]["skipped"][0]["path"],
        absent.to_str().unwrap()
    );
    assert!(
        report["targets"][0]["skipped"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("absent")
    );
    let target = temp.path().join("target");
    basic(&target);
    let result = temp.path().join("result.json");
    binary()
        .env_remove("ONEBUDGETSPEC_RESULT")
        .args(["--keep-within", "2d", "--dry-run"])
        .arg(&target)
        .assert()
        .success();
    assert!(!result.exists());
    binary()
        .env("ONEBUDGETSPEC_RESULT", &result)
        .args(["--keep-within", "2d"])
        .arg(&target)
        .assert()
        .success();
    let result: Value = serde_json::from_slice(&fs::read(result).unwrap()).unwrap();
    assert!(result["value"].is_f64());
    for field in [
        "entries walked:",
        "profile directories: 1",
        "builds removed: 2",
        "bytes freed:",
        "seconds walking:",
        "seconds removing:",
    ] {
        assert!(result["detail"].as_str().unwrap().contains(field));
    }
    expect_removed(&target.join("debug"), "alpha", A);
    expect_removed(&target.join("debug"), "beta", A);
}

#[test]
fn identical_hashes_do_not_merge_distinct_unit_names() {
    let temp = tempdir();
    let profile = temp.path().join("debug");
    build(&profile, "alpha", A, OLD);
    build(&profile, "alpha", B, MIDDLE);
    build(&profile, "beta", B, OLD);
    build(&profile, "beta", A, MIDDLE);
    let before = kept(&profile, &[("alpha", B), ("beta", A)]);
    let report = json(&[temp.path()], &[]);
    expect_removed(&profile, "alpha", A);
    expect_removed(&profile, "beta", B);
    assert_eq!(kept(&profile, &[("alpha", B), ("beta", A)]), before);
    assert_eq!(report["targets"][0]["builds_removed"], 2);
}

#[test]
fn malformed_build_names_survive_and_nonhex_incremental_ids_are_pruned() {
    let temp = tempdir();
    let profile = temp.path().join("debug");
    build(&profile, "liberty", A, OLD);
    build(&profile, "liberty", B, MIDDLE);
    let malformed = [
        "lib-0000000000000001.rlib",
        "libalpha-000000000000001.rlib",
        "libalpha-00000000000000001.rlib",
        "libalpha-000000000000000g.rlib",
        "libalpha-.rlib",
    ];
    let paths: Vec<_> = malformed
        .iter()
        .map(|name| profile.join("deps").join(name))
        .collect();
    for path in &paths {
        write(path);
        age_tree(path, OLD);
    }
    let before: Vec<_> = paths.iter().map(|p| inventory(p)).collect();
    let old_inc = profile.join("incremental/nonhex-older_id");
    let new_inc = profile.join("incremental/nonhex-newer_id");
    write(&old_inc.join("nested/leaf"));
    write(&new_inc.join("nested/leaf"));
    age_tree(&old_inc, OLD);
    age_tree(&new_inc, MIDDLE);
    let kept_inc = inventory(&new_inc);
    let kept_build = kept(&profile, &[("liberty", B)]);
    let result = json(&[temp.path()], &[]);
    assert_eq!(result["targets"][0]["builds_removed"], 1);
    let retained = du(&[
        super::fixture::paths(&profile, "liberty", B),
        vec![new_inc.clone()],
    ]
    .concat());
    assert_eq!(
        result["targets"][0]["bytes_retained"].as_u64().unwrap(),
        retained
    );
    expect_removed(&profile, "liberty", A);
    assert_eq!(kept(&profile, &[("liberty", B)]), kept_build);
    assert!(!old_inc.exists());
    assert_eq!(inventory(&new_inc), kept_inc);
    for (path, expected) in paths.iter().zip(before) {
        assert_eq!(inventory(path), expected);
    }
}

#[test]
fn telemetry_write_failure_diagnoses_error_after_completed_pruning() {
    let temp = tempdir();
    let target = temp.path().join("target");
    basic(&target);
    let output = binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&target)
        .env("ONEBUDGETSPEC_RESULT", temp.path())
        .assert()
        .success()
        .get_output()
        .clone();
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("budget report:")
    );
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["targets"][0]["builds_removed"], 2);
    expect_removed(&target.join("debug"), "alpha", A);
    expect_removed(&target.join("debug"), "beta", A);
}

#[test]
fn documentation_matches_compiled_cli_and_report_schema() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let docs = std::process::Command::new(env!("CARGO"))
        .current_dir(&root)
        .args(["test", "--locked", "-p", "cargo-prune-superseded", "--doc"])
        .output()
        .unwrap();
    assert!(
        docs.status.success(),
        "documented SDK consumer failed: {}",
        String::from_utf8_lossy(&docs.stderr)
    );
    let readme = fs::read_to_string(root.join("README.md")).unwrap();
    assert_eq!(
        readme,
        fs::read_to_string(root.join("crates/cargo-prune-superseded/README.md")).unwrap()
    );
    let workspace = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    let minimum = workspace
        .lines()
        .find_map(|line| {
            line.strip_prefix("rust-version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap();
    assert!(readme.contains(&format!("Rust {minimum} or newer")));
    let usage = readme
        .split("```text\n")
        .nth(1)
        .unwrap()
        .split("\n```")
        .next()
        .unwrap();
    let help = binary()
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(help).unwrap();
    for flag in usage
        .split_whitespace()
        .filter_map(|token| token.trim_matches(['[', ']']).strip_prefix("--"))
    {
        assert!(help.contains(&format!("--{flag}")));
    }
    assert!(
        help.lines()
            .find(|l| l.starts_with("Usage:"))
            .unwrap()
            .contains("--keep-within <KEEP_WITHIN>")
    );
    let documented: Value = serde_json::from_str(
        readme
            .split("```json\n")
            .nth(1)
            .unwrap()
            .split("\n```")
            .next()
            .unwrap(),
    )
    .unwrap();
    let temp = tempdir();
    basic(temp.path());
    let actual = json(&[temp.path()], &[]);
    let documented = documented["targets"][0].as_object().unwrap();
    let actual = actual["targets"][0].as_object().unwrap();
    assert_eq!(
        documented.keys().collect::<Vec<_>>(),
        actual.keys().collect::<Vec<_>>()
    );
    for (key, value) in documented {
        assert_eq!(
            (value.is_string(), value.is_u64(), value.is_array()),
            (
                actual[key].is_string(),
                actual[key].is_u64(),
                actual[key].is_array()
            )
        );
    }
    assert!(readme.contains("Exit codes are `0`"));
    assert!(readme.contains("`1` for an unreadable target"));
    assert!(readme.contains("`2` for usage"));
}

#[test]
fn retained_hardlinks_contribute_allocated_bytes_once() {
    let temp = tempdir();
    basic(temp.path());
    let profile = temp.path().join("debug");
    let source = paths(&profile, "alpha", B)[0].clone();
    let link = paths(&profile, "alpha", C)[0].clone();
    fs::remove_file(&link).unwrap();
    fs::hard_link(&source, &link).unwrap();
    let entries: Vec<_> = [("alpha", B), ("alpha", C), ("beta", B)]
        .iter()
        .flat_map(|(u, h)| paths(&profile, u, h))
        .collect();
    let measured = du(&entries);
    let before = kept(&profile, &[("alpha", B), ("alpha", C), ("beta", B)]);
    let dry = json(&[temp.path()], &["--dry-run"]);
    assert_eq!(
        dry["targets"][0]["bytes_retained"].as_u64().unwrap(),
        measured
    );
    let report = json(&[temp.path()], &[]);
    assert_eq!(
        report["targets"][0]["bytes_retained"].as_u64().unwrap(),
        measured
    );
    assert_eq!(
        kept(&profile, &[("alpha", B), ("alpha", C), ("beta", B)]),
        before
    );
    expect_removed(&profile, "alpha", A);
    expect_removed(&profile, "beta", A);
}

#[test]
fn hyphenated_package_and_rustc_spellings_share_the_whole_builds_newest_mtime() {
    let temp = tempdir();
    let profile = temp.path().join("debug");
    build(&profile, "some-unit", A, OLD);
    build(&profile, "some-unit", B, RECENT);
    // Only the otherwise old build’s package-spelled build/ entry has a recent
    // nested file. Its incremental directory is independently its unit’s newest.
    fs::remove_dir_all(super::fixture::paths(&profile, "some-unit", B)[4].clone()).unwrap();
    recent(
        &profile.join(format!("build/some-unit-{A}/one/two/leaf")),
        WINDOW,
    );
    build(&profile, "control", A, OLD);
    build(&profile, "control", B, MIDDLE);
    let before: Vec<_> = [A, B]
        .iter()
        .flat_map(|h| super::fixture::paths(&profile, "some-unit", h))
        .filter(|p| p.exists())
        .map(|path| {
            let contents = inventory(&path);
            (path, contents)
        })
        .collect();
    let result = json(&[temp.path()], &[]);
    for (path, contents) in before {
        assert_eq!(inventory(&path), contents);
    }
    expect_removed(&profile, "control", A);
    assert_eq!(result["targets"][0]["builds_removed"], 1);
}

#[test]
fn normalized_names_with_disjoint_hashes_keep_build_identities_separate() {
    let temp = tempdir();
    let profile = temp.path().join("debug");
    let fourth = "0000000000000004";
    build(&profile, "same-name", A, OLD);
    build(&profile, "same-name", B, WINDOW);
    build(&profile, "same_name", C, OLD);
    build(&profile, "same_name", fourth, RECENT);
    let before = kept(&profile, &[("same-name", B), ("same_name", fourth)]);
    let result = json(&[temp.path()], &[]);
    expect_removed(&profile, "same-name", A);
    expect_removed(&profile, "same_name", C);
    assert_eq!(
        kept(&profile, &[("same-name", B), ("same_name", fourth)]),
        before
    );
    assert_eq!(result["targets"][0]["builds_removed"], 2);
}

fn detail_number(result: &Value, label: &str) -> String {
    result["detail"]
        .as_str()
        .unwrap()
        .split("; ")
        .find_map(|field| field.strip_prefix(&format!("{label}: ")))
        .unwrap()
        .to_owned()
}

#[test]
fn multi_target_budget_telemetry_aggregates_the_whole_invocation() {
    let temp = tempdir();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    basic(&first);
    for (hash, age) in [(A, OLD), (B, WINDOW), (C, RECENT)] {
        build(&second.join("release"), "omega", hash, age);
    }
    let first_before = inventory(&first);
    let second_before = inventory(&second);
    let result_path = temp.path().join("result.json");
    let mut expected_walked = 0;
    for target in [&first, &second] {
        binary()
            .args(["--keep-within", "2d", "--json", "--dry-run"])
            .arg(target)
            .env("ONEBUDGETSPEC_RESULT", &result_path)
            .assert()
            .success();
        let result: Value = serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
        expected_walked += detail_number(&result, "entries walked")
            .parse::<u64>()
            .unwrap();
    }
    let dry_output = binary()
        .args(["--keep-within", "2d", "--json", "--dry-run"])
        .arg(&first)
        .arg(&second)
        .env("ONEBUDGETSPEC_RESULT", &result_path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dry: Value = serde_json::from_slice(&dry_output).unwrap();
    let telemetry: Value = serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
    assert_eq!(
        detail_number(&telemetry, "entries walked")
            .parse::<u64>()
            .unwrap(),
        expected_walked
    );
    assert_eq!(inventory(&first), first_before);
    assert_eq!(inventory(&second), second_before);
    let output = binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&first)
        .arg(&second)
        .env("ONEBUDGETSPEC_RESULT", &result_path)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let actual: Value = serde_json::from_slice(&output).unwrap();
    let telemetry: Value = serde_json::from_slice(&fs::read(&result_path).unwrap()).unwrap();
    for (label, field, expected) in [
        ("profile directories", "profiles", 2),
        ("builds removed", "builds_removed", 3),
    ] {
        let sum: u64 = actual["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t[field].as_u64().unwrap())
            .sum();
        assert_eq!(sum, expected);
        assert_eq!(
            detail_number(&telemetry, label).parse::<u64>().unwrap(),
            sum
        );
    }
    let freed: u64 = actual["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["bytes_freed"].as_u64().unwrap())
        .sum();
    assert_eq!(
        detail_number(&telemetry, "bytes freed")
            .parse::<u64>()
            .unwrap(),
        freed
    );
    let predicted: u64 = dry["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["bytes_freed"].as_u64().unwrap())
        .sum();
    assert_eq!(freed, predicted);
    // Timings are reported numeric figures; the workload budget judges their values,
    // not a correctness assertion on an elapsed-time bound or cost figure.
    assert!(telemetry["value"].is_f64());
    for field in ["seconds walking", "seconds removing"] {
        assert!(detail_number(&telemetry, field).parse::<f64>().is_ok());
    }
    expect_removed(&first.join("debug"), "alpha", A);
    expect_removed(&first.join("debug"), "beta", A);
    expect_removed(&second.join("release"), "omega", A);
}

#[test]
fn a_unit_named_liberty_keeps_dependency_files_and_bare_executables_with_its_build() {
    let temp = tempdir();
    let profile = temp.path().join("debug");
    build(&profile, "liberty", A, OLD);
    build(&profile, "liberty", B, RECENT);
    recent(
        &profile.join(format!("build/liberty-{A}/one/two/leaf")),
        WINDOW,
    );
    recent(
        &profile.join(format!("incremental/liberty-{A}/one/two/leaf")),
        WINDOW,
    );
    let executables: Vec<_> = [(A, OLD), (B, RECENT)]
        .iter()
        .map(|(hash, age)| {
            let path = profile.join(format!("deps/liberty-{hash}"));
            write(&path);
            age_tree(&path, *age);
            path
        })
        .collect();
    let mut artifacts = Vec::new();
    for (hash, age) in [(A, OLD), (B, RECENT)] {
        for extension in ["rmeta", "so", "dylib", "a"] {
            let path = profile.join(format!("deps/libliberty-{hash}.{extension}"));
            write(&path);
            age_tree(&path, age);
            artifacts.push(path);
        }
    }
    let artifact_before: Vec<_> = artifacts.iter().map(|p| inventory(p)).collect();
    let before = kept(&profile, &[("liberty", A), ("liberty", B)]);
    let binary_before: Vec<_> = executables.iter().map(|p| inventory(p)).collect();
    build(&profile, "control", A, OLD);
    build(&profile, "control", B, MIDDLE);
    let result = json(&[temp.path()], &[]);
    assert_eq!(kept(&profile, &[("liberty", A), ("liberty", B)]), before);
    for (path, contents) in executables.iter().zip(binary_before) {
        assert_eq!(inventory(path), contents);
    }
    for (path, contents) in artifacts.iter().zip(artifact_before) {
        assert_eq!(inventory(path), contents);
    }
    expect_removed(&profile, "control", A);
    assert_eq!(result["targets"][0]["builds_removed"], 1);
}

#[test]
fn absent_optional_containers_and_incremental_files_do_not_prevent_safe_pruning() {
    for container in ["deps", "build", "incremental"] {
        let temp = tempdir();
        basic(temp.path());
        let profile = temp.path().join("debug");
        fs::remove_dir_all(profile.join(container)).unwrap();
        let file = profile.join(format!("incremental/fileunit-{A}"));
        if container != "incremental" {
            write(&file);
            age_tree(&file, OLD);
            let sibling = profile.join(format!("incremental/fileunit-{B}"));
            write(&sibling.join("nested/leaf"));
            age_tree(&sibling, MIDDLE);
        }
        let file_before = (container != "incremental").then(|| inventory(&file));
        let before: Vec<_> = [("alpha", B), ("alpha", C), ("beta", B)]
            .iter()
            .flat_map(|(u, h)| paths(&profile, u, h))
            .filter(|p| p.exists())
            .map(|path| {
                let data = inventory(&path);
                (path, data)
            })
            .collect();
        let result = json(&[temp.path()], &[]);
        assert_eq!(result["targets"][0]["builds_removed"], 2);
        assert_eq!(result["targets"][0]["skipped"], serde_json::json!([]));
        for (path, data) in before {
            assert_eq!(inventory(&path), data);
        }
        for (unit, hash) in [("alpha", A), ("beta", A)] {
            for path in paths(&profile, unit, hash) {
                assert!(!path.exists(), "{} survived", path.display());
            }
        }
        if let Some(data) = file_before {
            assert_eq!(inventory(&file), data);
        }
    }
}
