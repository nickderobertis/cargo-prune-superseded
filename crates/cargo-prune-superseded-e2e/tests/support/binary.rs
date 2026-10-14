//! Build and spawn the real binary in this test executable’s own profile.
use assert_cmd::Command;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The built binary, in the target directory this test executable was built into
/// (`<target>/<profile>/deps/<test>`), where cargo puts a workspace member's binaries.
/// `cargo test` does not build another package's binary, so it is built here, once per
/// run, into the same target directory and profile with the environment this run
/// inherited (under `cargo llvm-cov` that is the instrumented build), so a journey never
/// drives a stale binary. It is a no-op when the binary is already up to date.
pub fn binary() -> Command {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    let path = BUILT.get_or_init(|| {
        let exe = std::env::current_exe().expect("the test executable's path");
        let profile_dir = exe
            .parent()
            .and_then(Path::parent)
            .expect("the test executable sits in <target>/<profile>/deps");
        let path = profile_dir.join(format!(
            "cargo-prune-superseded{}",
            std::env::consts::EXE_SUFFIX
        ));
        {
            let target_dir = profile_dir.parent().expect("<target>/<profile>");
            let mut build = std::process::Command::new(env!("CARGO"));
            build
                .args([
                    "build",
                    "--quiet",
                    "--locked",
                    "-p",
                    "cargo-prune-superseded",
                ])
                .arg("--target-dir")
                .arg(target_dir);
            if profile_dir
                .file_name()
                .is_some_and(|name| name == "release")
            {
                build.arg("--release");
            }
            let status = build.status().expect("cargo spawns");
            assert!(
                status.success() && path.is_file(),
                "building {} failed ({status}); run `cargo build -p cargo-prune-superseded` \
                 to see why, then re-run",
                path.display()
            );
        }
        path
    });
    Command::new(path)
}
