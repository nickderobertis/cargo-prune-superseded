//! Native Windows filesystem accounting and failure journeys, run by the install job.
#![cfg(windows)]

use std::collections::BTreeSet;
use std::fs;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use prune_fixtures::{
    A, B, C, OLD, age_tree, basic, expect_removed, inventory, kept, paths, tempdir,
};
use serde_json::Value;

#[path = "support/binary.rs"]
mod binary_driver;
use binary_driver::binary;

// Independent whole-tree accounting: count each native file identity once across
// the target and link directory, without reconstructing the pruner's selections.
fn allocated(roots: &[PathBuf]) -> u64 {
    fn walk(path: &Path, seen: &mut BTreeSet<(u64, u64)>) -> u64 {
        let meta = fs::symlink_metadata(path).unwrap();
        if meta.is_symlink() {
            return 0;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0200_0000 | 0x0020_0000)
            .open(path)
            .unwrap();
        let info = winapi_util::file::information(&file).unwrap();
        let mut bytes = if seen.insert((info.volume_serial_number(), info.file_index())) {
            file.allocated_size().unwrap()
        } else {
            0
        };
        if meta.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                bytes += walk(&entry.unwrap().path(), seen);
            }
        }
        bytes
    }
    let mut seen = BTreeSet::new();
    roots.iter().map(|path| walk(path, &mut seen)).sum()
}

#[test]
fn native_windows_directories_hardlinks_and_allocated_sizes_survive_dry_run_then_prune() {
    let temp = tempdir();
    let target = temp.path().join("target");
    basic(&target);
    let profile = target.join("debug");
    let oldest = paths(&profile, "alpha", A)[0].clone();
    fs::write(&oldest, b"old").unwrap();
    age_tree(&oldest, OLD);
    fs::write(&paths(&profile, "alpha", C)[0], b"new").unwrap();
    let links = temp.path().join("links");
    fs::create_dir(&links).unwrap();
    fs::hard_link(&oldest, links.join("artifact")).unwrap();
    let kept_units = [("alpha", B), ("alpha", C), ("beta", B)];
    let before_kept = kept(&profile, &kept_units);
    let before_outside = inventory(&links);
    let before = inventory(&target);
    let roots = [target.clone(), links.clone()];
    let before_bytes = allocated(&roots);
    let retained_paths: Vec<_> = kept_units
        .iter()
        .flat_map(|(u, h)| paths(&profile, u, h))
        .collect();
    let retained = allocated(&retained_paths);
    let removed_paths = [paths(&profile, "alpha", A), paths(&profile, "beta", A)].concat();
    let candidate_bytes = allocated(&removed_paths);
    let linked_bytes = allocated(&[oldest]);
    let dry = binary()
        .args(["--keep-within", "2d", "--json", "--dry-run"])
        .arg(&target)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let dry: Value = serde_json::from_slice(&dry).unwrap();
    assert_eq!(
        dry["targets"][0]["bytes_freed"].as_u64().unwrap(),
        candidate_bytes - linked_bytes
    );
    assert_eq!(
        dry["targets"][0]["bytes_retained"].as_u64().unwrap(),
        retained
    );
    assert_eq!(inventory(&target), before);
    let output = binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&target)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let result: Value = serde_json::from_slice(&output).unwrap();
    expect_removed(&profile, "alpha", A);
    expect_removed(&profile, "beta", A);
    assert_eq!(kept(&profile, &kept_units), before_kept);
    assert_eq!(inventory(&links), before_outside);
    assert_eq!(result["targets"][0]["builds_removed"], 2);
    assert_eq!(
        result["targets"][0]["bytes_freed"].as_u64().unwrap(),
        before_bytes - allocated(&roots)
    );
    assert_eq!(
        result["targets"][0]["bytes_retained"].as_u64().unwrap(),
        retained
    );
}

#[test]
fn windows_open_handle_refuses_deletion_and_other_builds_continue() {
    let temp = tempdir();
    let target = temp.path().join("target");
    basic(&target);
    let profile = target.join("debug");
    let blocked = paths(&profile, "alpha", A)[0].clone();
    // Share reads and writes so measurement succeeds, but deny delete sharing:
    // DeleteFile must fail while this actual kernel handle remains open.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(0x1 | 0x2)
        .open(&blocked)
        .unwrap();
    let output = binary()
        .args(["--keep-within", "2d", "--json"])
        .arg(&target)
        .output()
        .unwrap();
    drop(held);
    assert!(output.status.success());
    let result: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(blocked.exists());
    expect_removed(&profile, "beta", A);
    assert_eq!(result["targets"][0]["builds_removed"], 1);
    assert!(
        result["targets"][0]["skipped"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["path"] == blocked.to_str().unwrap()
                && s["reason"].as_str().is_some_and(|r| !r.is_empty()))
    );
}
