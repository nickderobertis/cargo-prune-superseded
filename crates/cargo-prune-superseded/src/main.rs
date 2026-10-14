//! Command-line shell over the pruning SDK.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use cargo_prune_superseded::{Options, Skipped, TargetReport, parse_span, prune_target_measured};
use clap::Parser;

#[derive(Clone, Copy, Debug)]
struct KeepWithin(Duration);
impl std::str::FromStr for KeepWithin {
    type Err = cargo_prune_superseded::Error;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        parse_span(text).map(Self)
    }
}

#[derive(Parser)]
#[command(name = "cargo-prune-superseded", version, about)]
struct Cli {
    /// Keep all builds modified within this positive span (e.g. 2d or 36h).
    #[arg(long, allow_hyphen_values = true)]
    keep_within: KeepWithin,
    /// Report what would be removed without removing anything.
    #[arg(long)]
    // llmlint: ignore[invalid_states_unrepresentable] This clap flag has exactly two valid states matching the fixed SDK dry_run bool; an enum would not remove an invalid state.
    dry_run: bool,
    /// Print a machine-readable JSON report.
    #[arg(long)]
    // llmlint: ignore[invalid_states_unrepresentable] This clap presence flag has exactly two valid output formats; neither bool value is invalid.
    json: bool,
    /// Cargo target directories, relative to the current directory; defaults to target.
    #[arg(value_name = "target-dir")]
    targets: Vec<PathBuf>,
}

#[derive(Clone, Copy)]
enum Action {
    Remove,
    WouldRemove,
}
impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::Remove => "removed",
            Self::WouldRemove => "would remove",
        }
    }
}

fn print_human(report: &TargetReport, action: Action) {
    println!(
        "cargo-prune-superseded: {}: {} {} superseded build(s) across {} profile(s), freed {} B, kept {} B",
        report.path.display(),
        action.label(),
        report.builds_removed,
        report.profiles,
        report.bytes_freed,
        report.bytes_retained
    );
    for skipped in &report.skipped {
        // llmlint: ignore[cli_output_contract] The fixed CLI contract requires these indented stdout lines after the summary; fatal and usage diagnostics use stderr.
        println!("  skipped {}: {}", skipped.path.display(), skipped.reason);
    }
}

fn main() {
    let start = Instant::now();
    let mut args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "prune-superseded") {
        args.remove(1);
    }
    let cli = Cli::parse_from(args);
    let options = Options {
        keep_within: cli.keep_within.0,
        dry_run: cli.dry_run,
    };
    let targets = if cli.targets.is_empty() {
        vec![PathBuf::from("target")]
    } else {
        cli.targets
    };
    let now = SystemTime::now();
    let mut reports = Vec::new();
    let mut walked = 0;
    let mut walking = 0.0;
    let mut removing = 0.0;
    let mut exit = 0;
    for target in targets {
        match prune_target_measured(&target, &options, now) {
            Ok((report, metrics)) => {
                walked += metrics.entries_walked;
                walking += metrics.walking_seconds;
                removing += metrics.removing_seconds;
                reports.push(report);
            }
            Err(error) => {
                eprintln!("cargo-prune-superseded: {error}");
                exit = 1;
                let reason = std::error::Error::source(&error)
                    .map_or_else(|| error.to_string(), ToString::to_string);
                reports.push(TargetReport {
                    path: target.clone(),
                    profiles: 0,
                    builds_removed: 0,
                    bytes_freed: 0,
                    bytes_retained: 0,
                    skipped: vec![Skipped {
                        path: target,
                        reason,
                    }],
                });
            }
        }
    }
    if cli.json {
        println!("{}", serde_json::json!({"targets": reports}));
    } else {
        let action = if cli.dry_run {
            Action::WouldRemove
        } else {
            Action::Remove
        };
        for report in &reports {
            print_human(report, action);
        }
    }
    let profiles: u32 = reports.iter().map(|r| r.profiles).sum();
    let builds: u64 = reports.iter().map(|r| r.builds_removed).sum();
    let bytes: u64 = reports.iter().map(|r| r.bytes_freed).sum();
    let detail = format!(
        "entries walked: {walked}; profile directories: {profiles}; builds removed: {builds}; bytes freed: {bytes}; seconds walking: {walking}; seconds removing: {removing}"
    );
    if let Err(error) = onebudgetspec_core::report(start.elapsed().as_secs_f64(), Some(&detail)) {
        eprintln!("cargo-prune-superseded: budget report: {error}");
    }
    std::process::exit(exit);
}
