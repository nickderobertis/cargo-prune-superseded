//! `scripts/session-setup.sh` and `scripts/setup-llmlint.sh`, run as the `SessionStart` hook
//! runs them: a subprocess in a sandbox whose HOME is a temporary directory and whose
//! PATH holds only the basic tools and what a journey puts there: the real `just`,
//! `cargo` and `bun`, linked from this test's own PATH, where a journey needs them present,
//! and stubs for `uv` and `llmlint`, which install from and call out to the network.

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

fn script_path(name: &str) -> PathBuf {
    script(name)
}

fn script(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../scripts")
        .join(name)
}

fn executable(path: &Path, body: &str) {
    std::fs::write(path, body).expect("an executable file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .expect("an executable bit");
}

/// HOME, a PATH of the basic tools plus a stub directory, and a session env file.
struct Sandbox {
    home: tempfile::TempDir,
    tools: tempfile::TempDir,
    stubs: tempfile::TempDir,
    env_file: PathBuf,
}

impl Sandbox {
    fn new() -> Self {
        let home = tempfile::tempdir().expect("a HOME");
        let tools = tempfile::tempdir().expect("a tools directory");
        for tool in ["bash", "dirname", "chmod", "mkdir", "cat"] {
            let real = ["/usr/bin", "/bin"]
                .iter()
                .map(|dir| Path::new(dir).join(tool))
                .find(|path| path.is_file())
                .unwrap_or_else(|| panic!("{tool} is installed"));
            std::os::unix::fs::symlink(real, tools.path().join(tool)).expect("a tool link");
        }
        let env_file = home.path().join("claude-env");
        Self {
            home,
            tools,
            stubs: tempfile::tempdir().expect("a stub directory"),
            env_file,
        }
    }

    fn bin_dir(&self) -> PathBuf {
        self.home.path().join(".local/bin")
    }

    /// The real `tool`, from this test's own PATH, linked into the sandbox.
    fn present(&self, tool: &str) -> &Self {
        let real = std::env::var_os("PATH")
            .iter()
            .flat_map(std::env::split_paths)
            .map(|dir| dir.join(tool))
            .find(|path| path.is_file())
            .unwrap_or_else(|| panic!("{tool} must be installed to run this journey"));
        std::os::unix::fs::symlink(real, self.stubs.path().join(tool)).expect("a tool link");
        self
    }

    /// A `uv` that records its arguments and, when `installs` is true, "installs" the
    /// tool it was asked for into `~/.local/bin` as a working stub, else exits 1.
    // llmlint: ignore-block[e2e_not_mocked] uv installs from PyPI over the network, which this offline tier cannot reach; the stub stands only for that boundary, and the provisioners' own decisions run for real.
    fn uv(&self, installs: bool) -> &Self {
        let log = self.home.path().join("uv-calls");
        let body = if installs {
            format!(
                "#!/usr/bin/env bash\necho \"$*\" >>{log:?}\nmkdir -p \"$HOME/.local/bin\"\n\
                 case \"$*\" in\n\
                 *rust-just*) printf '#!/usr/bin/env bash\\nexit 0\\n' >\"$HOME/.local/bin/just\"; chmod +x \"$HOME/.local/bin/just\" ;;\n\
                 *llmlint-cli*) printf '#!/usr/bin/env bash\\nexit 0\\n' >\"$HOME/.local/bin/llmlint\"; chmod +x \"$HOME/.local/bin/llmlint\" ;;\n\
                 esac\n",
                log = log.display().to_string()
            )
        } else {
            format!(
                "#!/usr/bin/env bash\necho \"$*\" >>{log:?}\necho 'error: network unreachable' >&2\nexit 1\n",
                log = log.display().to_string()
            )
        };
        executable(&self.stubs.path().join("uv"), &body);
        self
    }
    // llmlint: ignore-end[e2e_not_mocked]

    // llmlint: ignore-block[e2e_not_mocked] a real `llmlint doctor` probes an authenticated coding harness, which this offline tier has none of; these stubs stand only for its two answers.
    /// An `llmlint` whose `doctor` succeeds and says so on stdout.
    fn llmlint_doctor_passing_loudly(&self) -> &Self {
        executable(
            &self.stubs.path().join("llmlint"),
            "#!/usr/bin/env bash\necho 'oneharness: claude-code ready'\nexit 0\n",
        );
        self
    }

    /// An `llmlint` whose `doctor` fails, saying why.
    fn llmlint_doctor_failing(&self) -> &Self {
        executable(
            &self.stubs.path().join("llmlint"),
            "#!/usr/bin/env bash\n[ \"$1\" = doctor ] && { echo 'oneharness: no harness authenticated'; exit 1; }\nexit 0\n",
        );
        self
    }
    // llmlint: ignore-end[e2e_not_mocked]

    fn uv_calls(&self) -> Vec<String> {
        std::fs::read_to_string(self.home.path().join("uv-calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn env_file(&self) -> String {
        std::fs::read_to_string(&self.env_file).unwrap_or_default()
    }

    fn path(&self) -> String {
        format!(
            "{}:{}",
            self.stubs.path().display(),
            self.tools.path().display()
        )
    }

    fn run(&self, name: &str, extra: &[(&str, &str)]) -> Output {
        let mut command = std::process::Command::new(script(name));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", self.path())
            .env("CLAUDE_ENV_FILE", &self.env_file)
            .current_dir(self.home.path());
        for (key, value) in extra {
            command.env(key, value);
        }
        command.output_retrying().expect("the script runs")
    }
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

fn assert_quiet_success(output: &Output) {
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(output));
    assert_eq!(output.stdout, b"", "a provisioner prints nothing on stdout");
    assert_eq!(stderr(output), "", "a provisioner is silent on success");
}

#[test]
fn session_setup_does_nothing_in_ci() {
    let sandbox = Sandbox::new();
    sandbox.uv(true);
    let output = sandbox.run("session-setup.sh", &[("CI", "true")]);
    assert_quiet_success(&output);
    assert_eq!(sandbox.uv_calls(), Vec::<String>::new());
    assert_eq!(sandbox.env_file(), "");
}

#[test]
fn session_setup_installs_just_and_llmlint_and_persists_path_quietly() {
    let sandbox = Sandbox::new();
    sandbox.uv(true).present("cargo").present("bun");
    let output = sandbox.run("session-setup.sh", &[]);
    assert_quiet_success(&output);
    assert_eq!(
        sandbox.uv_calls(),
        [
            "tool install --quiet --upgrade rust-just>=1.51.0",
            "tool install --quiet --upgrade llmlint-cli>=0.3.23",
        ]
    );
    assert!(
        sandbox.bin_dir().join("just").is_file(),
        "just was installed"
    );
    assert!(
        sandbox
            .env_file()
            .contains(&format!("export PATH={}:", sandbox.bin_dir().display())),
        "PATH is persisted for the session: {}",
        sandbox.env_file()
    );
}

#[test]
fn a_later_shell_sourcing_the_session_env_file_resolves_the_installed_tools() {
    let sandbox = Sandbox::new();
    // A HOME whose path needs shell quoting, as the persisted line must survive it.
    let home = sandbox.home.path().join("my home's dir");
    std::fs::create_dir(&home).expect("a HOME needing quoting");
    sandbox.uv(true).present("cargo").present("bun");
    let mut command = std::process::Command::new(script("session-setup.sh"));
    let output = command
        .env_clear()
        .env("HOME", &home)
        .env("PATH", sandbox.path())
        .env("CLAUDE_ENV_FILE", &sandbox.env_file)
        .output_retrying()
        .expect("session-setup runs");
    assert_quiet_success(&output);
    let later = std::process::Command::new(sandbox.tools.path().join("bash"))
        .args([
            "-c",
            "source \"$1\" && command -v just && command -v llmlint",
            "bash",
        ])
        .arg(&sandbox.env_file)
        .env_clear()
        .env("PATH", sandbox.path())
        .output_retrying()
        .expect("a later shell runs");
    assert_eq!(later.status.code(), Some(0), "stderr: {}", stderr(&later));
    let bin = home.join(".local/bin");
    assert_eq!(
        String::from_utf8(later.stdout).expect("utf-8"),
        format!("{}/just\n{}/llmlint\n", bin.display(), bin.display())
    );
}

#[test]
fn setup_llmlint_alone_persists_a_path_a_later_shell_can_source() {
    let sandbox = Sandbox::new();
    sandbox.uv(true);
    let output = sandbox.run("setup-llmlint.sh", &[]);
    assert_quiet_success(&output);
    let later = std::process::Command::new(sandbox.tools.path().join("bash"))
        .args(["-c", "source \"$1\" && command -v llmlint", "bash"])
        .arg(&sandbox.env_file)
        .env_clear()
        .env("PATH", sandbox.path())
        .output_retrying()
        .expect("a later shell runs");
    assert_eq!(later.status.code(), Some(0), "stderr: {}", stderr(&later));
    assert_eq!(
        String::from_utf8(later.stdout).expect("utf-8"),
        format!("{}/llmlint\n", sandbox.bin_dir().display())
    );
}

#[test]
fn an_unset_or_unusable_home_is_reported_and_installs_nothing() {
    let sandbox = Sandbox::new();
    sandbox.uv(true);
    let file = sandbox.home.path().join("a-file");
    std::fs::write(&file, "").expect("a regular file");
    let missing = sandbox.home.path().join("missing");
    let homes = [None, Some(file.as_path()), Some(missing.as_path())];
    for (home, (script, prefix)) in homes.iter().flat_map(|home| {
        [
            ("session-setup.sh", "session-setup"),
            ("setup-llmlint.sh", "setup-llmlint"),
        ]
        .map(|pair| (home, pair))
    }) {
        let mut command = std::process::Command::new(script_path(script));
        command.env_clear().env("PATH", sandbox.path());
        if let Some(home) = home {
            command.env("HOME", home);
        }
        let output = command.output_retrying().expect("the script runs");
        assert_eq!(output.status.code(), Some(0), "startup is never blocked");
        assert!(
            stderr(&output).contains(&format!("{prefix}: HOME is unset or not a directory")),
            "stderr: {}",
            stderr(&output)
        );
    }
    assert_eq!(sandbox.uv_calls(), Vec::<String>::new());
}

#[test]
fn session_setup_leaves_an_existing_just_alone() {
    let sandbox = Sandbox::new();
    sandbox
        .uv(true)
        .present("just")
        .present("cargo")
        .present("bun");
    let output = sandbox.run("session-setup.sh", &[]);
    assert_quiet_success(&output);
    assert_eq!(
        sandbox.uv_calls(),
        ["tool install --quiet --upgrade llmlint-cli>=0.3.23"]
    );
}

#[test]
fn session_setup_does_not_persist_a_path_that_already_holds_the_tool_directory() {
    let sandbox = Sandbox::new();
    sandbox
        .uv(true)
        .present("just")
        .present("cargo")
        .present("bun");
    let path = format!("{}:{}", sandbox.bin_dir().display(), sandbox.path());
    let output = sandbox.run("session-setup.sh", &[("PATH", &path)]);
    assert_quiet_success(&output);
    assert_eq!(sandbox.env_file(), "");
}

#[test]
fn session_setup_reports_a_failed_install_and_still_succeeds() {
    let sandbox = Sandbox::new();
    sandbox.uv(false).present("cargo").present("bun");
    let output = sandbox.run("session-setup.sh", &[]);
    assert_eq!(output.status.code(), Some(0), "startup is never blocked");
    let stderr = stderr(&output);
    assert!(
        stderr.contains("rust-just install failed"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("llmlint-cli install failed"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("llmlint is not on PATH"),
        "stderr: {stderr}"
    );
}

#[test]
fn session_setup_names_every_missing_tool_and_what_to_do() {
    let sandbox = Sandbox::new();
    let output = sandbox.run("session-setup.sh", &[]);
    assert_eq!(output.status.code(), Some(0), "startup is never blocked");
    let stderr = stderr(&output);
    for expected in [
        "neither just nor uv is installed; install uv",
        "cargo is not on PATH",
        "bun is not on PATH",
        "setup-llmlint: uv is not installed",
    ] {
        assert!(
            stderr.contains(expected),
            "stderr must say {expected:?}: {stderr}"
        );
    }
}

#[test]
fn setup_llmlint_installs_quietly() {
    let sandbox = Sandbox::new();
    sandbox.uv(true);
    let output = sandbox.run("setup-llmlint.sh", &[]);
    assert_quiet_success(&output);
    assert_eq!(
        sandbox.uv_calls(),
        ["tool install --quiet --upgrade llmlint-cli>=0.3.23"]
    );
    assert!(sandbox.bin_dir().join("llmlint").is_file());
}

#[test]
fn setup_llmlint_shows_a_failing_doctor_and_what_to_do() {
    let sandbox = Sandbox::new();
    sandbox.uv(false).llmlint_doctor_failing();
    let output = sandbox.run("setup-llmlint.sh", &[]);
    assert_eq!(output.status.code(), Some(0), "startup is never blocked");
    let stderr = stderr(&output);
    assert!(
        stderr.contains("oneharness: no harness authenticated"),
        "stderr: {stderr}"
    );
    assert!(
        stderr.contains("llmlint doctor reported an issue"),
        "stderr: {stderr}"
    );
}

#[test]
fn session_setup_without_a_session_env_file_persists_nothing_and_succeeds() {
    let sandbox = Sandbox::new();
    sandbox.uv(true).present("cargo").present("bun");
    let output = sandbox.run("session-setup.sh", &[("CLAUDE_ENV_FILE", "")]);
    assert_quiet_success(&output);
    assert_eq!(sandbox.env_file(), "");
}

#[test]
fn an_unwritable_session_env_file_is_reported_with_what_to_do() {
    let sandbox = Sandbox::new();
    sandbox.uv(true).present("cargo").present("bun");
    let unwritable = sandbox.home.path().join("no-such-dir/claude-env");
    let unwritable = unwritable.to_str().expect("a utf-8 path");
    for (script, prefix) in [
        ("session-setup.sh", "session-setup"),
        ("setup-llmlint.sh", "setup-llmlint"),
    ] {
        let output = sandbox.run(script, &[("CLAUDE_ENV_FILE", unwritable)]);
        assert_eq!(output.status.code(), Some(0), "startup is never blocked");
        let expected = format!(
            "{prefix}: could not write PATH to the session env file {unwritable} \
             (No such file or directory); check that it is writable"
        );
        assert!(
            stderr(&output).contains(&expected),
            "stderr: {}",
            stderr(&output)
        );
    }
}

/// session-setup.sh copied alone into a directory, without its setup-llmlint.sh.
fn session_setup_alone() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("a scripts directory");
    let copy = dir.path().join("session-setup.sh");
    std::fs::copy(script("session-setup.sh"), &copy).expect("a copy of session-setup.sh");
    (dir, copy)
}

fn run_copy(sandbox: &Sandbox, copy: &Path) -> Output {
    std::process::Command::new(copy)
        .env_clear()
        .env("HOME", sandbox.home.path())
        .env("PATH", sandbox.path())
        .current_dir(sandbox.home.path())
        .output_retrying()
        .expect("the copy runs")
}

#[test]
fn session_setup_reports_a_missing_llmlint_installer() {
    let sandbox = Sandbox::new();
    sandbox.present("just").present("cargo").present("bun");
    let (_dir, copy) = session_setup_alone();
    let output = run_copy(&sandbox, &copy);
    assert_eq!(output.status.code(), Some(0), "startup is never blocked");
    assert!(
        stderr(&output)
            .contains("setup-llmlint.sh is missing or not executable; restore it from git"),
        "stderr: {}",
        stderr(&output)
    );
}

const PROVISIONERS: [&str; 2] = ["session-setup.sh", "setup-llmlint.sh"];

#[test]
fn a_second_run_persists_nothing_new() {
    for script in PROVISIONERS {
        let sandbox = Sandbox::new();
        sandbox.uv(true).present("cargo").present("bun");
        assert_quiet_success(&sandbox.run(script, &[]));
        let first = sandbox.env_file();
        assert_quiet_success(&sandbox.run(script, &[]));
        assert_eq!(
            sandbox.env_file(),
            first,
            "{script}: a re-run appends no second PATH line"
        );
        assert_eq!(first.lines().count(), 1, "{script}");
    }
}

#[test]
fn a_session_env_file_that_becomes_writable_is_written_on_the_next_run() {
    for script in PROVISIONERS {
        let sandbox = Sandbox::new();
        sandbox.uv(true).present("cargo").present("bun");
        let dir = sandbox.home.path().join("session");
        let env_file = dir.join("env");
        let env_file = env_file.to_str().expect("a utf-8 path");
        let failed = sandbox.run(script, &[("CLAUDE_ENV_FILE", env_file)]);
        assert!(
            stderr(&failed).contains("could not write PATH"),
            "{script}: {}",
            stderr(&failed)
        );
        std::fs::create_dir(&dir).expect("the session directory now exists");
        assert_quiet_success(&sandbox.run(script, &[("CLAUDE_ENV_FILE", env_file)]));
        assert!(
            std::fs::read_to_string(env_file)
                .expect("the env file was written")
                .starts_with("export PATH="),
            "{script}"
        );
    }
}

#[test]
fn session_setup_installs_just_on_the_run_after_a_failed_install() {
    let sandbox = Sandbox::new();
    sandbox.uv(false).present("cargo").present("bun");
    let failed = sandbox.run("session-setup.sh", &[]);
    assert_eq!(failed.status.code(), Some(0), "startup is never blocked");
    assert!(
        stderr(&failed).contains("rust-just install failed"),
        "stderr: {}",
        stderr(&failed)
    );
    assert!(!sandbox.bin_dir().join("just").exists());
    sandbox.uv(true);
    assert_quiet_success(&sandbox.run("session-setup.sh", &[]));
    assert!(
        sandbox.bin_dir().join("just").is_file(),
        "just is installed on the retry"
    );
}

#[test]
fn setup_llmlint_recovers_on_the_run_after_a_failed_install() {
    let sandbox = Sandbox::new();
    sandbox.uv(false);
    let failed = sandbox.run("setup-llmlint.sh", &[]);
    assert!(
        stderr(&failed).contains("llmlint-cli install failed"),
        "stderr: {}",
        stderr(&failed)
    );
    sandbox.uv(true);
    assert_quiet_success(&sandbox.run("setup-llmlint.sh", &[]));
    assert!(sandbox.bin_dir().join("llmlint").is_file());
}

#[test]
fn a_passing_doctor_stays_silent() {
    let sandbox = Sandbox::new();
    sandbox.uv(false).llmlint_doctor_passing_loudly();
    let output = sandbox.run("setup-llmlint.sh", &[]);
    assert_eq!(output.status.code(), Some(0));
    let stderr = stderr(&output);
    assert!(
        !stderr.contains("claude-code ready"),
        "doctor's success is not echoed: {stderr}"
    );
    assert_eq!(output.stdout, b"");
}

#[test]
fn session_setup_reports_an_installer_that_cannot_run() {
    let sandbox = Sandbox::new();
    sandbox.present("just").present("cargo").present("bun");
    let (dir, copy) = session_setup_alone();
    // The real installer with Windows line endings, as a checkout without
    // `* text=auto eol=lf` would leave it: its interpreter line names `bash\r`.
    let installer = std::fs::read_to_string(script("setup-llmlint.sh"))
        .expect("the installer")
        .replace('\n', "\r\n");
    executable(&dir.path().join("setup-llmlint.sh"), &installer);
    let output = run_copy(&sandbox, &copy);
    assert_eq!(output.status.code(), Some(0), "startup is never blocked");
    assert!(
        stderr(&output).contains("setup-llmlint.sh could not run (above); restore it from git"),
        "stderr: {}",
        stderr(&output)
    );
}

#[test]
fn a_repaired_doctor_clears_the_diagnostic_on_the_next_run() {
    let sandbox = Sandbox::new();
    sandbox.uv(false).llmlint_doctor_failing();
    let failing = sandbox.run("setup-llmlint.sh", &[]);
    assert!(
        stderr(&failing).contains("llmlint doctor reported an issue"),
        "stderr: {}",
        stderr(&failing)
    );
    sandbox.llmlint_doctor_passing_loudly();
    let repaired = sandbox.run("setup-llmlint.sh", &[]);
    assert_eq!(repaired.status.code(), Some(0));
    assert!(
        !stderr(&repaired).contains("llmlint doctor reported an issue"),
        "stderr: {}",
        stderr(&repaired)
    );
}
