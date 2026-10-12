//! `scripts/check-boundaries.mjs`, run with node as the `workspace:lint` target runs it,
//! in a scratch directory whose `node_modules/.bin/nx` is a stub that writes a chosen
//! project graph: the graph Nx computes is the input under test, so each journey hands
//! the script one and asserts its verdict, exit status and message.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

fn run_with_graph(graph: Option<&str>) -> Output {
    let workspace = tempfile::tempdir().expect("a workspace");
    let bin = workspace.path().join("node_modules/.bin");
    std::fs::create_dir_all(&bin).expect("node_modules/.bin");
    let served = workspace.path().join("graph.json");
    // llmlint: ignore-block[e2e_not_mocked] the graph Nx computes is this script's input, so a stub nx serving a chosen graph is how each verdict is reached; the script itself runs for real under node.
    let body = match graph {
        Some(graph) => {
            std::fs::write(&served, graph).expect("a graph");
            format!(
                "#!/usr/bin/env bash\nfor arg in \"$@\"; do case \"$arg\" in --file=*) cp {served:?} \"${{arg#--file=}}\";; esac; done\n",
                served = served.display().to_string()
            )
        }
        None => {
            "#!/usr/bin/env bash\necho 'NX Could not find project.json' >&2\nexit 1\n".to_owned()
        }
    };
    // llmlint: ignore-end[e2e_not_mocked]
    let nx = bin.join("nx");
    std::fs::write(&nx, body).expect("the nx stub");
    std::fs::set_permissions(&nx, std::fs::Permissions::from_mode(0o755)).expect("+x");
    std::process::Command::new("node")
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/check-boundaries.mjs"))
        .current_dir(workspace.path())
        .output()
        .expect("node runs the script")
}

/// An Nx graph document of `projects` (name, tag) and `edges` (source, target).
fn graph(projects: &[(&str, &str)], edges: &[(&str, &str)]) -> String {
    let nodes: Vec<String> = projects
        .iter()
        .map(|(name, tag)| format!(r#""{name}":{{"data":{{"tags":["{tag}"]}}}}"#))
        .collect();
    let dependencies: Vec<String> = projects
        .iter()
        .map(|(name, _)| {
            let targets: Vec<String> = edges
                .iter()
                .filter(|(source, _)| source == name)
                .map(|(_, target)| format!(r#"{{"target":"{target}"}}"#))
                .collect();
            format!(r#""{name}":[{}]"#, targets.join(","))
        })
        .collect();
    format!(
        r#"{{"graph":{{"nodes":{{{}}},"dependencies":{{{}}}}}}}"#,
        nodes.join(","),
        dependencies.join(",")
    )
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

const PROJECTS: &[(&str, &str)] = &[
    ("app", "type:app"),
    ("app-e2e", "type:e2e"),
    ("tooling", "type:tooling"),
];

#[test]
fn a_graph_whose_edges_point_from_tests_to_what_they_check_passes_quietly() {
    let output = run_with_graph(Some(&graph(
        PROJECTS,
        &[("app-e2e", "app"), ("app-e2e", "tooling")],
    )));
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(output.stdout, b"");
    assert_eq!(stderr(&output), "");
}

#[test]
fn an_edge_onto_a_test_suite_fails_naming_it() {
    let output = run_with_graph(Some(&graph(PROJECTS, &[("tooling", "app-e2e")])));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("tooling -> app-e2e (any may not depend on type:e2e)"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn the_deliverable_depending_on_tooling_fails_naming_the_edge() {
    let output = run_with_graph(Some(&graph(PROJECTS, &[("app", "tooling")])));
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("app -> tooling (type:app may not depend on type:tooling)")
            && stderr(&output).contains("remove the edge"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_graph_nx_cannot_compute_is_reported_with_what_to_do() {
    let output = run_with_graph(None);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output)
            .contains("could not read the Nx project graph: NX Could not find project.json")
            && stderr(&output).contains("run 'just bootstrap'"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_malformed_graph_is_refused() {
    for (graph, reason) in [
        (
            r#"{"graph":{"nodes":[],"dependencies":{}}}"#,
            "has no nodes or dependencies map",
        ),
        (
            r#"{"graph":{"nodes":{"a":{"data":{"tags":["type:app"]}}},"dependencies":{"a":"b"}}}"#,
            "lists a's dependencies as string, not a list",
        ),
        (
            r#"{"graph":{"nodes":{"a":{"data":{"tags":["type:app"]}},"b":{"data":{}}},"dependencies":{"a":[{"target":"b"}]}}}"#,
            "a depends on b, which has no tags list of strings",
        ),
        (
            r#"{"graph":{"nodes":{"a":{"data":{}}},"dependencies":{"a":[]}}}"#,
            "a has no tags list of strings",
        ),
    ] {
        let output = run_with_graph(Some(graph));
        assert_eq!(output.status.code(), Some(1), "graph: {graph}");
        assert!(
            stderr(&output).contains(reason),
            "stderr: {}",
            stderr(&output)
        );
    }
}

/// The graph shape the script reads is Nx's own: the real `nx graph` over this workspace
/// (installed by `just bootstrap`) must write a graph whose nodes carry tag lists and
/// whose dependencies are lists of `{target}` edges, and the script must pass it.
#[test]
fn the_real_nx_graph_has_the_shape_the_script_reads_and_passes() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let nx = root.join("node_modules/.bin/nx");
    assert!(nx.is_file(), "run 'just bootstrap' so Nx is installed");
    let out = tempfile::tempdir().expect("a directory for the graph");
    let file = out.path().join("graph.json");
    let status = std::process::Command::new(&nx)
        .arg("graph")
        .arg(format!("--file={}", file.display()))
        .current_dir(&root)
        .env("NX_DAEMON", "false")
        .stdout(std::process::Stdio::null())
        .status()
        .expect("nx runs");
    assert!(status.success(), "nx graph failed");
    let graph: serde_norway::Value =
        serde_norway::from_str(&std::fs::read_to_string(&file).expect("the graph")).expect("JSON");
    let nodes = graph["graph"]["nodes"].as_mapping().expect("a nodes map");
    assert!(nodes.contains_key("cargo-prune-superseded"));
    for (name, node) in nodes {
        assert!(
            node["data"]["tags"]
                .as_sequence()
                .is_some_and(|tags| tags.iter().all(serde_norway::Value::is_string)),
            "{name:?} has no tags list"
        );
    }
    let dependencies = graph["graph"]["dependencies"]
        .as_mapping()
        .expect("a dependencies map");
    let e2e = dependencies[&serde_norway::Value::from("cargo-prune-superseded-e2e")]
        .as_sequence()
        .expect("a list of edges");
    assert!(
        e2e.iter()
            .any(|edge| edge["target"].as_str() == Some("cargo-prune-superseded")),
        "the e2e project's edge onto the binary is a {{target}} entry"
    );
    let verdict = std::process::Command::new("node")
        .arg(root.join("scripts/check-boundaries.mjs"))
        .current_dir(&root)
        .output()
        .expect("node runs the script");
    assert_eq!(
        verdict.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&verdict)
    );
}
