//! The real Nx task graphs keep release workloads out of every ordinary gate selection.
#![cfg(unix)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn tasks(workspace: &Path, args: &[&str]) -> Vec<String> {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("graph.json");
    let output = Command::new(root().join("node_modules/.bin/nx"))
        .current_dir(workspace)
        .env("NX_DAEMON", "false")
        .args(args)
        .arg(format!("--graph={}", path.display()))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let graph: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    graph["tasks"]["tasks"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect()
}

fn fixture() -> (tempfile::TempDir, String, String, String) {
    let scratch = tempfile::tempdir().unwrap();
    let files = Command::new("git")
        .current_dir(root())
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .output()
        .unwrap();
    assert!(files.status.success());
    for relative in files
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative = std::str::from_utf8(relative).unwrap();
        let destination = scratch.path().join(relative);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::copy(root().join(relative), destination).unwrap();
    }
    std::os::unix::fs::symlink(
        root().join("node_modules"),
        scratch.path().join("node_modules"),
    )
    .unwrap();
    git(scratch.path(), &["init", "--quiet"]);
    git(scratch.path(), &["config", "user.name", "Graph fixture"]);
    git(
        scratch.path(),
        &["config", "user.email", "graph-fixture@example.invalid"],
    );
    git(scratch.path(), &["add", "-A"]);
    git(
        scratch.path(),
        &["commit", "--quiet", "-m", "chore: graph fixture baseline"],
    );
    let base = git(scratch.path(), &["rev-parse", "HEAD"]);
    let source = scratch
        .path()
        .join("crates/cargo-prune-superseded/src/lib.rs");
    std::fs::write(
        &source,
        std::fs::read_to_string(&source).unwrap() + "\n// Source-only graph fixture change.\n",
    )
    .unwrap();
    git(scratch.path(), &["add", "-A"]);
    git(
        scratch.path(),
        &["commit", "--quiet", "-m", "chore: change pruning source"],
    );
    let source_head = git(scratch.path(), &["rev-parse", "HEAD"]);
    let journey = scratch
        .path()
        .join("crates/cargo-prune-superseded/tests/workload/tests/journeys.rs");
    std::fs::write(
        &journey,
        std::fs::read_to_string(&journey).unwrap() + "\n// Workload-only graph fixture change.\n",
    )
    .unwrap();
    git(scratch.path(), &["add", "-A"]);
    git(
        scratch.path(),
        &["commit", "--quiet", "-m", "chore: change workload journey"],
    );
    let journey_head = git(scratch.path(), &["rev-parse", "HEAD"]);
    (scratch, base, source_head, journey_head)
}

fn git(path: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn release_workload_has_its_own_edge_and_is_absent_from_gate_graphs() {
    let (scratch, base, source_head, journey_head) = fixture();
    let source_base_arg = format!("--base={base}");
    let source_head_arg = format!("--head={source_head}");
    let journey_base_arg = format!("--base={source_head}");
    let journey_head_arg = format!("--head={journey_head}");
    let targets = "format-check,lint,typecheck,test,build,budgets,coverage";
    for args in [
        vec!["run-many", "-t", targets],
        vec![
            "affected",
            &source_base_arg,
            &source_head_arg,
            "-t",
            targets,
        ],
        vec![
            "affected",
            &journey_base_arg,
            &journey_head_arg,
            "-t",
            targets,
        ],
    ] {
        let graph = tasks(scratch.path(), &args);
        assert!(
            !graph.iter().any(|task| task.ends_with(":workload")),
            "unexpected workload in {graph:?}"
        );
    }
    assert_eq!(
        tasks(scratch.path(), &["run-many", "-t", "workload"]),
        ["cargo-prune-superseded-workload:workload"]
    );
    let workflow = std::fs::read_to_string(root().join(".github/workflows/ci.yml")).unwrap();
    assert!(workflow.contains("      - run: just check all\n      - run: just workload"));
    let recipe = std::fs::read_to_string(root().join("justfile")).unwrap();
    assert!(recipe.contains("workload:\n    bunx nx run-many -t workload"));
}
