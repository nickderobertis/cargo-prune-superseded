//! `scripts/install-smoke.sh`, run from a scratch copy of the repository layout with a
//! recording `cargo` first on PATH whose `install` puts a chosen binary in the install
//! root: the real package-and-install path is the gate's own `cargo-prune-superseded-install`
//! run and the CI install job, so here each journey drives one of the script's verdicts.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

/// How the stub toolchain and the binary it installs behave in one journey.
#[derive(Clone, Copy)]
struct Behaviour {
    failing: Option<&'static str>,
    pkgid: &'static str,
    lock_install_root: bool,
    version_output: &'static str,
    version_exit: u8,
    help_output: &'static str,
}

const WORKING: Behaviour = Behaviour {
    failing: None,
    pkgid: "path+file:///repo/crates/cargo-prune-superseded#0.1.0",
    lock_install_root: false,
    version_output: "cargo-prune-superseded 0.1.0",
    version_exit: 0,
    help_output: "Usage: cargo-prune-superseded",
};

fn run(behaviour: Behaviour, tmpdir: Option<&str>) -> (Output, Vec<String>) {
    let (output, calls, _) = run_in(behaviour, tmpdir);
    (output, calls)
}

/// As `run`, also returning the temporary directory the script was given, so a journey
/// can see what it left there.
fn run_in(behaviour: Behaviour, tmpdir: Option<&str>) -> (Output, Vec<String>, tempfile::TempDir) {
    let own_tmp = tempfile::tempdir().expect("a TMPDIR");
    let root = tempfile::tempdir().expect("a checkout");
    std::fs::create_dir(root.path().join("scripts")).expect("scripts/");
    let script = root.path().join("scripts/install-smoke.sh");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/install-smoke.sh"),
        &script,
    )
    .expect("the script");
    let bin = tempfile::tempdir().expect("a PATH directory");
    let log = root.path().join("calls");
    let failure = behaviour.failing.map_or(String::new(), |subcommand| {
        format!(
            "[ \"$1\" = {subcommand} ] && {{ echo 'error: {subcommand} broke' >&2; exit 101; }}\n"
        )
    });
    let installed = root.path().join("installed-binary");
    std::fs::write(
        &installed,
        format!(
            "#!/usr/bin/env bash\ncase \"$1\" in\n--version) echo '{}'; exit {} ;;\n--help) echo '{}' ;;\nesac\n",
            behaviour.version_output, behaviour.version_exit, behaviour.help_output
        ),
    )
    .expect("the binary cargo install will put in place");
    // llmlint: ignore-block[e2e_not_mocked] packaging and installing build the crate from source, which the gate's real install run and CI's install job already prove; the recording cargo stands only for that boundary so each of the script's verdicts can be reached.
    let cargo = bin.path().join("cargo");
    std::fs::write(
        &cargo,
        format!(
            "#!/usr/bin/env bash\necho \"$*\" >>{log:?}\n{failure}\
             case \"$1\" in\n\
             pkgid) echo {pkgid:?} ;;\n\
             install) root=\"$5\"; mkdir -p \"$root/bin\"; cp {installed:?} \"$root/bin/cargo-prune-superseded\"; chmod +x \"$root/bin/cargo-prune-superseded\"{lock} ;;\n\
             esac\nexit 0\n",
            log = log.display().to_string(),
            installed = installed.display().to_string(),
            pkgid = behaviour.pkgid,
            lock = if behaviour.lock_install_root { "; chmod a-w \"$root/bin\"" } else { "" },
        ),
    )
    .expect("the cargo stub");
    // llmlint: ignore-end[e2e_not_mocked]
    std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).expect("+x");
    let mut command = std::process::Command::new(&script);
    command.env("PATH", format!("{}:/usr/bin:/bin", bin.path().display()));
    match tmpdir {
        Some(tmpdir) => command.env("TMPDIR", tmpdir),
        None => command.env("TMPDIR", own_tmp.path()),
    };
    let mut attempts = 0;
    let output = loop {
        match command.output() {
            // ETXTBSY: another test thread forked while a stub was being written.
            Err(error) if error.raw_os_error() == Some(26) && attempts < 100 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => break result.expect("the script runs"),
        }
    };
    let calls = std::fs::read_to_string(&log)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    (output, calls, own_tmp)
}

fn stderr(output: &Output) -> &str {
    std::str::from_utf8(&output.stderr).expect("utf-8 stderr")
}

fn assert_refused(output: &Output, reason: &str) {
    assert_eq!(output.status.code(), Some(1), "stderr: {}", stderr(output));
    assert!(
        stderr(output).contains(reason),
        "stderr must say {reason:?}: {}",
        stderr(output)
    );
    assert!(
        stderr(output).contains("install-smoke: "),
        "it names itself"
    );
}

#[test]
fn a_working_install_passes_quietly_through_package_then_install() {
    let (output, calls) = run(WORKING, None);
    assert_eq!(output.status.code(), Some(0), "stderr: {}", stderr(&output));
    assert_eq!(output.stdout, b"");
    assert_eq!(stderr(&output), "");
    assert_eq!(calls[0], "pkgid -p cargo-prune-superseded");
    assert_eq!(
        calls[1],
        "package --locked --offline --allow-dirty -p cargo-prune-superseded"
    );
    assert!(
        calls[2].starts_with("install --locked --offline --root ")
            && calls[2].ends_with(" --path target/package/cargo-prune-superseded-0.1.0"),
        "install from the packaged archive: {}",
        calls[2]
    );
}

#[test]
fn each_failing_cargo_step_is_named_with_what_to_do() {
    for (subcommand, reason) in [
        ("pkgid", "cannot read cargo-prune-superseded's version"),
        ("package", "'cargo package' failed (above)"),
        ("install", "installing the packaged crate failed (above)"),
    ] {
        let (output, _) = run(
            Behaviour {
                failing: Some(subcommand),
                ..WORKING
            },
            None,
        );
        assert_refused(&output, reason);
        assert!(stderr(&output).contains(&format!("error: {subcommand} broke")));
    }
}

#[test]
fn an_installed_binary_that_misreports_is_refused() {
    for (behaviour, reason) in [
        (
            Behaviour {
                version_output: "prune 9.9.9",
                ..WORKING
            },
            "--version printed 'prune 9.9.9', expected 'cargo-prune-superseded 0.1.0'",
        ),
        (
            Behaviour {
                version_exit: 1,
                ..WORKING
            },
            "the installed binary's --version exited non-zero",
        ),
        (
            Behaviour {
                help_output: "nothing useful",
                ..WORKING
            },
            "--help printed no usage naming cargo-prune-superseded",
        ),
    ] {
        let (output, _) = run(behaviour, None);
        assert_refused(&output, reason);
    }
}

#[test]
fn an_unusable_temporary_directory_is_named_with_what_to_do() {
    let (output, calls) = run(WORKING, Some("/nonexistent/tmp"));
    assert_refused(&output, "cannot create a temporary install root");
    assert!(calls.iter().all(|call| !call.starts_with("install")));
}

fn left_in(tmp: &tempfile::TempDir) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(tmp.path())
        .expect("the TMPDIR")
        .map(|entry| entry.expect("an entry").path())
        .collect()
}

#[test]
fn the_install_root_is_removed_after_success_and_after_failure() {
    let (passed, _, tmp) = run_in(WORKING, None);
    assert_eq!(passed.status.code(), Some(0), "stderr: {}", stderr(&passed));
    assert_eq!(left_in(&tmp), Vec::<std::path::PathBuf>::new());
    let (failed, _, tmp) = run_in(
        Behaviour {
            help_output: "nothing useful",
            ..WORKING
        },
        None,
    );
    assert_eq!(failed.status.code(), Some(1));
    assert_eq!(left_in(&tmp), Vec::<std::path::PathBuf>::new());
}

#[test]
fn an_install_root_that_cannot_be_removed_fails_the_smoke() {
    let (output, _, tmp) = run_in(
        Behaviour {
            lock_install_root: true,
            ..WORKING
        },
        None,
    );
    for root in left_in(&tmp) {
        std::fs::set_permissions(root.join("bin"), std::fs::Permissions::from_mode(0o755))
            .expect("unlock the install root so the test can clean up");
    }
    assert_refused(&output, "could not remove");
    assert!(stderr(&output).contains("remove it by hand"));
}

#[test]
fn a_package_id_without_a_plain_version_is_refused() {
    let (output, calls) = run(
        Behaviour {
            pkgid: "path+file:///repo/crates/cargo-prune-superseded#0.1.0-rc.1/../x",
            ..WORKING
        },
        None,
    );
    assert_refused(&output, "ends in no plain X.Y.Z version");
    assert_eq!(calls, ["pkgid -p cargo-prune-superseded"]);
}
