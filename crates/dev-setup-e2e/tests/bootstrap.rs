//! `scripts/bootstrap.sh`, run as `just bootstrap` runs it (after `just fetch`), with `bun`, `cargo` and
//! `rustup` stubbed: each would install from the network, which this offline tier cannot
//! reach. The stubs record every call, so each journey asserts exactly what bootstrap
//! asked of them; the script's own decisions run for real.

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

fn bootstrap() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/bootstrap.sh")
}

/// A PATH of the basic tools plus recording stubs, and the log they append to.
struct Installers {
    dir: tempfile::TempDir,
}

impl Installers {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("a stub directory");
        for tool in ["bash", "dirname"] {
            let real = ["/usr/bin", "/bin"]
                .iter()
                .map(|bin| Path::new(bin).join(tool))
                .find(|path| path.is_file())
                .unwrap_or_else(|| panic!("{tool} is installed"));
            std::os::unix::fs::symlink(real, dir.path().join(tool)).expect("a tool link");
        }
        Self { dir }
    }

    /// A stub `tool` that records its call and exits 0, or prints `failure` and exits 101
    /// when called with `failing_args`.
    // llmlint: ignore-block[e2e_not_mocked] bun, cargo and rustup install toolchains and crates from the network, which this offline tier cannot reach; each stub stands only for that boundary and records what bootstrap asked of it, while the script's own decisions run for real.
    fn stub(&self, tool: &str, failing_args: Option<(&str, &str)>) -> &Self {
        let log = self.dir.path().join("calls");
        let failure = failing_args.map_or(String::new(), |(args, message)| {
            format!("[ \"$*\" = {args:?} ] && {{ echo {message:?} >&2; exit 101; }}\n")
        });
        let path = self.dir.path().join(tool);
        std::fs::write(
            &path,
            format!(
                "#!/usr/bin/env bash\necho \"{tool} $*\" >>{log:?}\n{failure}exit 0\n",
                log = log.display().to_string()
            ),
        )
        .expect("a stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("+x");
        self
    }

    /// A `cargo` that records its calls and, for `cargo install --locked <tool>`, puts a
    /// working `<tool>` on PATH — unless `<tool>` is `failing_tool`, which it refuses.
    fn installing_cargo(&self, failing_tool: Option<&str>) -> &Self {
        let log = self.dir.path().join("calls");
        let refuse = failing_tool.map_or(String::new(), |tool| {
            format!("[ \"$3\" = {tool:?} ] && {{ echo 'error: could not compile {tool}' >&2; exit 101; }}\n")
        });
        let path = self.dir.path().join("cargo");
        std::fs::write(
            &path,
            format!(
                "#!/usr/bin/env bash\necho \"cargo $*\" >>{log:?}\n\
                 if [ \"$1\" = install ]; then\n{refuse}\
                 printf '#!/usr/bin/env bash\\nexit 0\\n' >{dir:?}/\"$3\"; chmod +x {dir:?}/\"$3\"\nfi\nexit 0\n",
                log = log.display().to_string(),
                dir = self.dir.path().display().to_string(),
            ),
        )
        .expect("a stub");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("+x");
        self
    }
    // llmlint: ignore-end[e2e_not_mocked]

    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.dir.path().join("calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn run(&self) -> Output {
        let home = tempfile::tempdir().expect("a HOME");
        std::process::Command::new(bootstrap())
            .env_clear()
            .env("HOME", home.path())
            .env("PATH", self.dir.path())
            .output_retrying()
            .expect("bootstrap runs")
    }
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

#[test]
fn bootstrap_installs_the_locked_toolchain_quietly() {
    let installers = Installers::new();
    installers
        .stub("bun", None)
        .stub("cargo", None)
        .stub("rustup", None)
        .stub("cargo-llvm-cov", None)
        .stub("cargo-nextest", None);
    let output = installers.run();
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(output.stdout, b"");
    assert_eq!(stderr(&output), "");
    assert_eq!(
        installers.calls(),
        ["bun install --frozen-lockfile", "rustup toolchain install"]
    );
}

#[test]
fn bootstrap_installs_missing_cargo_test_tools() {
    let installers = Installers::new();
    installers.stub("bun", None).stub("cargo", None);
    let output = installers.run();
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(
        installers.calls(),
        [
            "bun install --frozen-lockfile",
            "cargo install --locked cargo-llvm-cov",
            "cargo install --locked cargo-nextest",
        ]
    );
}

#[test]
fn every_failing_step_stops_bootstrap_with_its_output_and_what_to_do() {
    for (tool, args, present_tools) in [
        ("bun", "install --frozen-lockfile", true),
        ("rustup", "toolchain install", true),
        ("cargo", "install --locked cargo-llvm-cov", false),
        ("cargo", "install --locked cargo-nextest", false),
    ] {
        let installers = Installers::new();
        for stubbed in ["bun", "rustup", "cargo"] {
            let failing = (stubbed == tool).then_some((args, "error: network unreachable"));
            installers.stub(stubbed, failing);
        }
        if present_tools {
            installers
                .stub("cargo-llvm-cov", None)
                .stub("cargo-nextest", None);
        }
        let output = installers.run();
        assert_eq!(
            output.status.code(),
            Some(1),
            "{tool} {args} must stop bootstrap"
        );
        let stderr = stderr(&output);
        assert!(
            stderr.contains("error: network unreachable"),
            "stderr: {stderr}"
        );
        assert!(
            stderr.contains(&format!(
                "bootstrap: '{tool} {args}' failed (above); fix it, then re-run 'just bootstrap'."
            )),
            "stderr: {stderr}"
        );
        assert_eq!(
            installers.calls().last().map(String::as_str),
            Some(format!("{tool} {args}").as_str()),
            "nothing runs after the failing step"
        );
    }
}

#[test]
fn a_missing_toolchain_is_named_with_where_to_get_it() {
    for (missing, present) in [("bun", "cargo"), ("cargo", "bun")] {
        let installers = Installers::new();
        installers.stub(present, None);
        let output = installers.run();
        assert_eq!(output.status.code(), Some(1));
        assert!(
            stderr(&output).contains(&format!(
                "bootstrap: {missing} is not installed; install it"
            )),
            "stderr: {}",
            stderr(&output)
        );
        assert_eq!(
            installers.calls(),
            Vec::<String>::new(),
            "nothing is installed"
        );
    }
}

#[test]
fn a_rerun_after_a_failed_install_completes_and_installs_nothing_twice() {
    let installers = Installers::new();
    installers
        .stub("bun", None)
        .stub("cargo", None)
        .stub(
            "rustup",
            Some(("toolchain install", "error: network unreachable")),
        )
        .stub("cargo-llvm-cov", None)
        .stub("cargo-nextest", None);
    let failed = installers.run();
    assert_eq!(failed.status.code(), Some(1));
    assert!(
        stderr(&failed).contains("'rustup toolchain install' failed"),
        "stderr: {}",
        stderr(&failed)
    );
    installers.stub("rustup", None);
    let retried = installers.run();
    assert_eq!(
        retried.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&retried)
    );
    assert_eq!(stderr(&retried), "");
    let rerun = installers.run();
    assert_eq!(rerun.status.code(), Some(0), "stderr: {}", stderr(&rerun));
    assert_eq!(
        installers.calls(),
        [
            "bun install --frozen-lockfile",
            "rustup toolchain install",
            "bun install --frozen-lockfile",
            "rustup toolchain install",
            "bun install --frozen-lockfile",
            "rustup toolchain install",
        ],
        "each run re-applies the locked installs and installs no cargo tool already present"
    );
}

#[test]
fn installed_cargo_tools_are_skipped_on_the_next_run_even_after_a_partial_failure() {
    let installers = Installers::new();
    installers
        .stub("bun", None)
        .stub("rustup", None)
        .installing_cargo(Some("cargo-nextest"));
    let partial = installers.run();
    assert_eq!(partial.status.code(), Some(1));
    assert!(
        stderr(&partial).contains("'cargo install --locked cargo-nextest' failed"),
        "stderr: {}",
        stderr(&partial)
    );
    installers.installing_cargo(None);
    let recovered = installers.run();
    assert_eq!(
        recovered.status.code(),
        Some(0),
        "stderr: {}",
        stderr(&recovered)
    );
    let rerun = installers.run();
    assert_eq!(rerun.status.code(), Some(0), "stderr: {}", stderr(&rerun));
    let installs: Vec<String> = installers
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("cargo install"))
        .collect();
    assert_eq!(
        installs,
        [
            "cargo install --locked cargo-llvm-cov",
            "cargo install --locked cargo-nextest",
            "cargo install --locked cargo-nextest",
        ],
        "llvm-cov installs once; nextest is retried once, then skipped"
    );
}
