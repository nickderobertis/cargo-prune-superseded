//! Release-only journeys: actual Cargo metadata hashes and a large sparse target.
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_json::Value;

use fixture::{A, B, C, MIDDLE, OLD, RECENT, WINDOW, age_tree, inventory, refresh_atime};
use prune_fixtures as fixture;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..")
}
fn binary() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let status = Command::new(env!("CARGO"))
                .current_dir(root())
                .args(["build", "--locked", "-p", "cargo-prune-superseded"])
                .status()
                .unwrap();
            assert!(status.success());
            root().join("target/debug/cargo-prune-superseded")
        })
        .clone()
}

fn cargo(project: &Path, features: &[&str]) {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.current_dir(project)
        .args(["build", "--offline", "--workspace"]);
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn hashes(profile: &Path, unit: &str) -> BTreeSet<String> {
    fs::read_dir(profile.join("deps"))
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().to_str().unwrap().to_owned();
            name.strip_prefix(&format!("lib{unit}-"))
                .and_then(|s| s.strip_suffix(".rlib"))
                .map(str::to_owned)
        })
        .collect()
}

fn unit_paths(profile: &Path, unit: &str, hash: &str) -> Vec<PathBuf> {
    ["deps", ".fingerprint", "build"]
        .iter()
        .flat_map(|container| {
            fs::read_dir(profile.join(container))
                .unwrap()
                .filter_map(|entry| {
                    let path = entry.unwrap().path();
                    let name = path.file_name().unwrap().to_str().unwrap();
                    let stem = name
                        .strip_prefix("lib")
                        .unwrap_or(name)
                        .split('.')
                        .next()
                        .unwrap();
                    (stem == format!("{unit}-{hash}")).then_some(path)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn entries_for(profile: &Path, unit: &str, hash: &str) -> fixture::Inventory {
    unit_paths(profile, unit, hash)
        .into_iter()
        .flat_map(|path| {
            inventory(&path, true)
                .into_iter()
                .map(move |(relative, value)| (path.join(relative), value))
        })
        .collect()
}

#[test]
fn real_two_crate_workspace_retains_each_units_newest_metadata_hash() {
    let temp = fixture::tempdir();
    fs::write(
        temp.path().join("Cargo.toml"),
        "[workspace]\nresolver = '3'\nmembers = ['alpha', 'beta']\n",
    )
    .unwrap();
    for unit in ["alpha", "beta"] {
        fs::create_dir_all(temp.path().join(unit).join("src")).unwrap();
        fs::write(temp.path().join(unit).join("Cargo.toml"), format!("[package]\nname = '{unit}'\nversion = '0.0.0'\nedition = '2024'\n[features]\none = []\ntwo = []\n")).unwrap();
        fs::write(
            temp.path().join(unit).join("src/lib.rs"),
            "pub fn value() -> u32 { 42 }\n",
        )
        .unwrap();
    }
    cargo(temp.path(), &[]);
    let profile = temp.path().join("target/debug");
    let alpha_a = hashes(&profile, "alpha").into_iter().next().unwrap();
    let beta_a = hashes(&profile, "beta").into_iter().next().unwrap();
    cargo(temp.path(), &["alpha/one", "beta/one"]);
    let alpha_b = hashes(&profile, "alpha")
        .into_iter()
        .find(|h| h != &alpha_a)
        .unwrap();
    let beta_b = hashes(&profile, "beta")
        .into_iter()
        .find(|h| h != &beta_a)
        .unwrap();
    cargo(temp.path(), &["alpha/two", "beta/one"]);
    let alpha_c = hashes(&profile, "alpha")
        .into_iter()
        .find(|h| h != &alpha_a && h != &alpha_b)
        .unwrap();
    assert_eq!(hashes(&profile, "alpha").len(), 3);
    assert_eq!(hashes(&profile, "beta").len(), 2);
    for (unit, hash, age) in [
        ("alpha", &alpha_a, OLD),
        ("alpha", &alpha_b, WINDOW),
        ("alpha", &alpha_c, RECENT),
        ("beta", &beta_a, OLD),
        ("beta", &beta_b, MIDDLE),
    ] {
        for path in unit_paths(&profile, unit, hash) {
            age_tree(&path, age);
        }
    }
    refresh_atime(&temp.path().join("target"));
    let kept: Vec<_> = [("alpha", &alpha_b), ("alpha", &alpha_c), ("beta", &beta_b)]
        .iter()
        .map(|(u, h)| entries_for(&profile, u, h))
        .collect();
    let out = Command::new(binary())
        .args(["--keep-within", "2d", "--json"])
        .arg(temp.path().join("target"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        unit_paths(&profile, "alpha", &alpha_a),
        Vec::<PathBuf>::new()
    );
    assert_eq!(unit_paths(&profile, "beta", &beta_a), Vec::<PathBuf>::new());
    for ((unit, hash), before) in [("alpha", &alpha_b), ("alpha", &alpha_c), ("beta", &beta_b)]
        .iter()
        .zip(kept)
    {
        assert_eq!(entries_for(&profile, unit, hash), before);
    }
    let report: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["targets"][0]["builds_removed"], 2);
}

fn sparse(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::File::create(path).unwrap().set_len(8192).unwrap();
}

fn count(path: &Path) -> u64 {
    1 + if path.is_dir() {
        fs::read_dir(path)
            .unwrap()
            .map(|e| count(&e.unwrap().path()))
            .sum()
    } else {
        0
    }
}

#[test]
fn large_sparse_target_records_the_real_invocations_budget_telemetry() {
    let temp = fixture::tempdir();
    let target = temp.path().join("target");
    let profiles = [
        "debug",
        "release",
        "aarch64-unknown-linux-gnu/debug",
        "llvm-cov-target/debug",
    ];
    let hashes = [
        A,
        B,
        C,
        "0000000000000004",
        "0000000000000005",
        "0000000000000006",
    ];
    for profile in profiles {
        for unit in 0..350 {
            for (i, hash) in hashes.iter().enumerate() {
                let paths = fixture::paths(&target.join(profile), &format!("unit{unit}"), hash);
                for path in &paths[..2] {
                    sparse(path);
                }
                for path in &paths[2..] {
                    for leaf in ["sibling", "second", "one/two/leaf"] {
                        sparse(&path.join(leaf));
                    }
                }
                for path in paths {
                    age_tree(
                        &path,
                        if i < 3 {
                            OLD + u64::try_from(i).unwrap() * 3600
                        } else {
                            WINDOW - u64::try_from(i).unwrap() * 3600
                        },
                    );
                }
            }
        }
    }
    let entries = count(&target);
    // Entry and profile counts are workload contracts; elapsed time is a budget,
    // measured below and judged only by onebudgetspec.
    assert!((160_000..180_000).contains(&entries));
    let recording = temp.path().join("pass.json");
    let out = Command::new(binary())
        .args(["--keep-within", "2d", "--json"])
        .arg(&target)
        .env("ONEBUDGETSPEC_RESULT", &recording)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reports: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(reports["targets"][0]["profiles"], 4);
    assert_eq!(reports["targets"][0]["builds_removed"], 4200);
    for profile in profiles {
        fixture::expect_removed(&target.join(profile), "unit0", A);
        assert!(
            fixture::paths(&target.join(profile), "unit0", hashes[5])
                .iter()
                .all(|p| p.exists())
        );
    }
    let mut telemetry: Value = serde_json::from_slice(&fs::read(recording).unwrap()).unwrap();
    telemetry["generated_entries"] = entries.into();
    telemetry["generated_profiles"] = 4.into();
    let output = root().join("dist/prune-workload/telemetry/prune-pass.json");
    fs::create_dir_all(output.parent().unwrap()).unwrap();
    fs::write(output, serde_json::to_vec_pretty(&telemetry).unwrap()).unwrap();
}
