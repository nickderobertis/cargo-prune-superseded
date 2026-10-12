//! `scripts/release-probe.sh`, spawned directly through its declared interface with a
//! stub `curl` first on `PATH` standing in for crates.io (the one external service; this
//! suite is offline). Its three answers — a version on stdout, an empty stdout, a
//! non-zero exit with the reason on stderr — are asserted together with the exit status
//! every time, because "none published" and "not answered" must never be confused.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Output;

/// `Command::output`, retrying while the kernel reports the executable busy (ETXTBSY):
/// under `cargo test`'s threads, another test forking while this one wrote an executable
/// holds it open for writing until that child execs.
trait OutputRetrying {
    fn output_retrying(&mut self) -> std::io::Result<Output>;
}

impl OutputRetrying for std::process::Command {
    fn output_retrying(&mut self) -> std::io::Result<Output> {
        const ETXTBSY: i32 = 26;
        let mut attempts = 0;
        loop {
            match self.output() {
                Err(error) if error.raw_os_error() == Some(ETXTBSY) && attempts < 100 => {
                    attempts += 1;
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                result => return result,
            }
        }
    }
}

const TARGET: &str = "crate:cargo-prune-superseded";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("an executable file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("an executable bit");
}

/// A stub registry: a `curl` that records each URL it is asked for and answers `body`
/// with `status` the way `curl -w '\n%{http_code}'` does, or exits without answering.
struct Registry {
    dir: tempfile::TempDir,
}

impl Registry {
    fn answering(status: &str, body: &str) -> Self {
        Self::with_tail(
            body,
            &format!("cat \"$served\"\nprintf '\\n%s' '{status}'\n"),
        )
    }

    fn failing(exit: u8) -> Self {
        Self::with_tail(
            "",
            &format!("echo 'curl: (28) Operation timed out' >&2\nexit {exit}\n"),
        )
    }

    fn with_tail(body: &str, tail: &str) -> Self {
        let dir = tempfile::tempdir().expect("a directory for the stub registry");
        let served = dir.path().join("served");
        std::fs::write(&served, body).expect("the stub's body");
        executable(
            &dir.path().join("curl"),
            &format!(
                "#!/usr/bin/env bash\nset -eu\nserved={served:?}\n\
                 printf '%s\\n' \"$@\" >>{arguments:?}\n\
                 for arg in \"$@\"; do case \"$arg\" in https://*) echo \"$arg\" >>{asked:?};; esac; done\n{tail}",
                served = served.display().to_string(),
                arguments = dir.path().join("arguments").display().to_string(),
                asked = dir.path().join("asked").display().to_string(),
            ),
        );
        Self { dir }
    }

    /// The repository's own probe, spawned directly (no shell) from an unrelated
    /// directory with only PATH and HOME — the environment a consumer spawns it with.
    fn ask(&self, identifier: &str) -> Output {
        self.spawn(
            &repository().join("scripts/release-probe.sh"),
            &[identifier],
        )
    }

    /// A copy of the probe in a checkout whose `release-targets.toml` is `declaration`
    /// (absent when `None`).
    fn ask_in_checkout(&self, declaration: Option<&str>, identifier: &str) -> Output {
        let checkout = tempfile::tempdir().expect("a checkout");
        std::fs::create_dir(checkout.path().join("scripts")).expect("scripts/");
        let probe = checkout.path().join("scripts/release-probe.sh");
        std::fs::copy(repository().join("scripts/release-probe.sh"), &probe).expect("the probe");
        if let Some(declaration) = declaration {
            std::fs::write(checkout.path().join("release-targets.toml"), declaration)
                .expect("the declaration");
        }
        self.spawn(&probe, &[identifier])
    }

    fn spawn(&self, probe: &Path, args: &[&str]) -> Output {
        Self::spawn_with_path(
            probe,
            args,
            &format!("{}:/usr/bin:/bin", self.dir.path().display()),
        )
    }

    fn spawn_with_path(probe: &Path, args: &[&str], path: &str) -> Output {
        let home = tempfile::tempdir().expect("a HOME");
        std::process::Command::new(probe)
            .args(args)
            .current_dir(home.path())
            .env_clear()
            .env("PATH", path)
            .env("HOME", home.path())
            .output_retrying()
            .expect("the probe spawns directly")
    }

    /// Every argument `curl` was given, in order.
    fn arguments(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("arguments"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn asked(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("asked"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

fn stdout(output: &Output) -> &str {
    std::str::from_utf8(&output.stdout).expect("utf-8 stdout")
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

/// A crate document as crates.io serves one, with `max_stable_version` set to `value`
/// (a JSON literal).
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] crates.io publishes no schema to derive this from; it copies the live v1 document's opening and field shape (https://crates.io/api/v1/crates/clap), the two things the probe reads, and the probe refuses any document that drifts from them.
fn crate_document(value: &str) -> String {
    format!(
        r#"{{"crate":{{"id":"cargo-prune-superseded","name":"cargo-prune-superseded","max_version":"0.4.0-rc.1","max_stable_version":{value},"newest_version":"0.4.0-rc.1"}},"versions":[]}}"#
    )
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// Asserts the "not answered" answer: a non-zero exit, nothing on stdout, and `reason`
/// on stderr.
fn assert_not_answered(output: &Output, reason: &str) {
    assert_ne!(output.status.code(), Some(0), "stdout: {}", stdout(output));
    assert_eq!(stdout(output), "", "a refusal must leave stdout empty");
    assert!(
        stderr(output).contains(reason),
        "stderr must say {reason:?}, got: {}",
        stderr(output)
    );
}

fn assert_none_published(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(output));
    assert_eq!(stdout(output), "");
}

#[test]
fn a_published_version_is_the_only_line_on_stdout() {
    let registry = Registry::answering("200", &crate_document(r#""0.3.1""#));
    let output = registry.ask(TARGET);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "0.3.1\n");
    assert_eq!(
        registry.asked(),
        ["https://crates.io/api/v1/crates/cargo-prune-superseded"]
    );
}

/// One bounded, unauthenticated read that ignores the caller's ~/.curlrc (`-q`, which
/// curl honours only first), names itself (crates.io refuses anonymous clients) and
/// frames the HTTP status after the body.
#[test]
fn the_registry_read_is_one_bounded_self_identifying_request() {
    let registry = Registry::answering("200", &crate_document(r#""0.3.1""#));
    let output = registry.ask(TARGET);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        registry.arguments(),
        [
            "-q",
            "-sS",
            "--connect-timeout",
            "10",
            "--max-time",
            "25",
            "-A",
            "cargo-prune-superseded-release-probe (+https://github.com/nickderobertis/cargo-prune-superseded)",
            "-w",
            r"\n%{http_code}",
            "--",
            "https://crates.io/api/v1/crates/cargo-prune-superseded",
        ]
    );
}

#[test]
fn a_crate_crates_io_does_not_know_is_none_published() {
    let registry = Registry::answering(
        "404",
        r#"{"errors":[{"detail":"crate `cargo-prune-superseded` does not exist"}]}"#,
    );
    assert_none_published(&registry.ask(TARGET));
}

#[test]
fn a_null_max_stable_version_is_none_published() {
    let registry = Registry::answering("200", &crate_document("null"));
    assert_none_published(&registry.ask(TARGET));
}

#[test]
fn an_unreadable_body_is_not_answered() {
    let registry = Registry::answering("200", "<html>maintenance</html>");
    assert_not_answered(&registry.ask(TARGET), "not a JSON object");
}

#[test]
fn a_document_about_another_crate_is_not_answered() {
    let registry = Registry::answering(
        "200",
        r#"{"crate":{"id":"serde","name":"serde","max_stable_version":"1.0.0"}}"#,
    );
    assert_not_answered(&registry.ask(TARGET), "a document that is not this crate's");
}

#[test]
fn a_document_without_exactly_one_max_stable_version_is_not_answered() {
    let missing = Registry::answering(
        "200",
        r#"{"crate":{"id":"cargo-prune-superseded","name":"cargo-prune-superseded"}}"#,
    );
    assert_not_answered(
        &missing.ask(TARGET),
        "without exactly one readable max_stable_version",
    );
    let twice = Registry::answering(
        "200",
        &crate_document(r#""1.0.0","extra":{"max_stable_version":"2.0.0"}"#),
    );
    assert_not_answered(
        &twice.ask(TARGET),
        "without exactly one readable max_stable_version",
    );
}

#[test]
fn a_value_that_is_not_a_stable_version_is_not_answered() {
    for value in [
        r#""1.2""#,
        r#""1.0.0-rc.1""#,
        r#""01.2.3""#,
        r#""1garbage""#,
    ] {
        let registry = Registry::answering("200", &crate_document(value));
        assert_not_answered(&registry.ask(TARGET), "not a stable semantic version");
    }
}

#[test]
fn a_registry_error_status_is_not_answered() {
    let registry = Registry::answering("503", "unavailable");
    assert_not_answered(&registry.ask(TARGET), "answered 503");
}

#[test]
fn a_reply_without_an_http_status_is_not_answered() {
    let registry = Registry::answering("abc", "{}");
    assert_not_answered(&registry.ask(TARGET), "rather than an HTTP status");
}

#[test]
fn a_failed_request_is_not_answered() {
    let registry = Registry::failing(28);
    assert_not_answered(&registry.ask(TARGET), "curl exited 28");
}

#[test]
fn an_identifier_the_declaration_does_not_name_is_not_answered() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    assert_not_answered(
        &registry.ask("crate:serde"),
        "'crate:serde' is not a release target this repository declares",
    );
    assert_eq!(
        registry.asked(),
        Vec::<String>::new(),
        "an undeclared identifier must reach no registry"
    );
}

#[test]
fn a_call_without_exactly_one_identifier_is_a_usage_error() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    let probe = repository().join("scripts/release-probe.sh");
    for args in [&[][..], &[TARGET, TARGET][..]] {
        let output = registry.spawn(&probe, args);
        assert_eq!(output.status.code(), Some(2), "stderr: {}", stderr(&output));
        assert_eq!(stdout(&output), "");
        assert!(stderr(&output).contains("takes exactly one registry-qualified identifier"));
    }
}

#[test]
fn a_host_without_curl_is_not_answered() {
    let bin = tempfile::tempdir().expect("a PATH without curl");
    for tool in ["bash", "grep"] {
        let real = ["/usr/bin", "/bin"]
            .iter()
            .map(|dir| Path::new(dir).join(tool))
            .find(|path| path.is_file())
            .expect("bash and grep are installed");
        std::os::unix::fs::symlink(real, bin.path().join(tool)).expect("a tool link");
    }
    let output = Registry::spawn_with_path(
        &repository().join("scripts/release-probe.sh"),
        &[TARGET],
        &bin.path().display().to_string(),
    );
    assert_not_answered(&output, "curl is not on PATH");
}

#[test]
fn a_checkout_without_a_declaration_is_not_answered() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    assert_not_answered(
        &registry.ask_in_checkout(None, TARGET),
        "missing or unreadable",
    );
    assert_eq!(registry.asked(), Vec::<String>::new());
}

#[test]
fn a_declaration_whose_id_is_not_a_quoted_string_is_not_answered() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    let output = registry.ask_in_checkout(
        Some("[[target]]\nid = crate:cargo-prune-superseded\n"),
        TARGET,
    );
    assert_not_answered(&output, "which is not a quoted string");
}

#[test]
fn a_declared_target_on_another_registry_is_not_answered() {
    let registry = Registry::answering("200", r#"{"info":{"version":"1.0.0"}}"#);
    let output = registry.ask_in_checkout(Some("[[target]]\nid = \"pypi:thing\"\n"), "pypi:thing");
    assert_not_answered(&output, "names no registry this probe reads");
    assert_eq!(registry.asked(), Vec::<String>::new());
}

#[test]
fn a_declared_name_crates_io_could_not_serve_is_not_answered() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    let output = registry.ask_in_checkout(Some("[[target]]\nid = \"crate:a.b\"\n"), "crate:a.b");
    assert_not_answered(&output, "not a name crates.io serves");
    assert_eq!(registry.asked(), Vec::<String>::new());
}

#[test]
fn a_declared_id_that_is_not_registry_and_name_is_not_answered() {
    for (declared, reason) in [
        ("crate:../x", "which is not <registry>:<name>"),
        (
            "crate:first crate:second",
            "holds a character no registry name has",
        ),
    ] {
        let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
        let declaration = format!("[[target]]\nid = \"{declared}\"\n");
        let output = registry.ask_in_checkout(Some(&declaration), "crate:second");
        assert_not_answered(&output, reason);
        assert_eq!(registry.asked(), Vec::<String>::new());
    }
}

#[test]
fn a_stable_version_with_build_metadata_is_answered_verbatim() {
    let registry = Registry::answering("200", &crate_document(r#""1.2.3+build.5""#));
    let output = registry.ask(TARGET);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "1.2.3+build.5\n");
}

#[test]
fn a_host_without_grep_is_not_answered() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    let bin = tempfile::tempdir().expect("a PATH without grep");
    let bash = ["/usr/bin/bash", "/bin/bash"]
        .iter()
        .map(Path::new)
        .find(|path| path.is_file())
        .expect("bash is installed");
    std::os::unix::fs::symlink(bash, bin.path().join("bash")).expect("a bash link");
    std::fs::copy(registry.dir.path().join("curl"), bin.path().join("curl"))
        .expect("the curl stub");
    let output = Registry::spawn_with_path(
        &repository().join("scripts/release-probe.sh"),
        &[TARGET],
        &bin.path().display().to_string(),
    );
    assert_not_answered(&output, "grep is not on PATH");
}

/// A declaration with comments, other tables and keys, a retired entry, and two targets.
const BUSY_DECLARATION: &str = r#"# What this checkout releases.
schema_version = 1
probe = "scripts/release-probe.sh"

[meta]
id = "crate:not-a-target"

[[target]]
id = "crate:first"   # the first target
name = "first"

[[retired]]
id = "crate:old"

[[target]]
name = "second"
id = "crate:second"
"#;

#[test]
fn every_declared_target_and_nothing_else_is_answered() {
    let second = Registry::answering(
        "200",
        r#"{"crate":{"id":"second","name":"second","max_stable_version":"2.0.0"}}"#,
    );
    let output = second.ask_in_checkout(Some(BUSY_DECLARATION), "crate:second");
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "2.0.0\n");
    let first = Registry::answering(
        "200",
        r#"{"crate":{"id":"first","name":"first","max_stable_version":"1.0.0"}}"#,
    );
    let output = first.ask_in_checkout(Some(BUSY_DECLARATION), "crate:first");
    assert_eq!(stdout(&output), "1.0.0\n", "stderr: {}", stderr(&output));
    for undeclared in ["crate:not-a-target", "crate:old"] {
        let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
        let output = registry.ask_in_checkout(Some(BUSY_DECLARATION), undeclared);
        assert_not_answered(&output, "is not a release target this repository declares");
        assert_eq!(registry.asked(), Vec::<String>::new());
    }
}

#[test]
fn a_declared_id_followed_by_anything_but_a_comment_is_not_answered() {
    let registry = Registry::answering("200", &crate_document(r#""1.0.0""#));
    let output = registry.ask_in_checkout(
        Some("[[target]]\nid = \"crate:cargo-prune-superseded\" trailing\n"),
        TARGET,
    );
    assert_not_answered(&output, "after the string");
}

#[test]
fn a_declaration_whose_last_line_has_no_newline_is_read() {
    let registry = Registry::answering("200", &crate_document(r#""0.3.1""#));
    let output = registry.ask_in_checkout(
        Some("[[target]]\nid = \"crate:cargo-prune-superseded\""),
        TARGET,
    );
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(stdout(&output), "0.3.1\n");
}
