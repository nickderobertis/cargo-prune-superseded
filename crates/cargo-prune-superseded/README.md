# cargo-prune-superseded

Delete superseded Cargo build output by **modification time**, retaining each compiled
unit's newest build and every build modified within a caller-chosen window.

## Install and run

Installing from source requires Rust 1.97 or newer.

```sh
CARGO_PRUNE_SUPERSEDED_VERSION='<published version>'
cargo install --locked --version "=$CARGO_PRUNE_SUPERSEDED_VERSION" cargo-prune-superseded
cargo-prune-superseded --keep-within 2d --dry-run target
cargo prune-superseded --keep-within 36h --json target other/target
```

Choose a published version and pin its exact version in each repository’s install
recipe.

```text
cargo-prune-superseded --keep-within <span> [--dry-run] [--json] [<target-dir>...]
```

`--keep-within` is required: a positive integer followed by `s`, `m`, `h` or `d`.
Zero, negative, fractional, unknown-unit and overflowing spans are usage errors.
Target arguments resolve from the current directory; without arguments, the tool uses
`target`. Missing targets are reported and skipped. A symlink in the target path is refused.

Each target produces a summary like:

```text
cargo-prune-superseded: target: removed 2 superseded build(s) across 1 profile(s), freed 32768 B, kept 65536 B
  skipped target/debug/deps/example: Permission denied (os error 13)
```

Dry-run says `would remove`, calculates the same selection and allocated sizes, and
changes nothing. `--json` emits:

```json
{"targets":[{"path":"target","profiles":1,"builds_removed":2,"bytes_freed":32768,"bytes_retained":65536,"skipped":[]}]}
```

Each skipped entry has `path` and `reason` strings. JSON renders non-UTF-8 path bytes
with replacement characters, as human output does. Exit codes are `0` for completed
passes (including skipped entries), `1` for an unreadable target, and `2` for usage
errors. An unreadable target does not prevent processing other named targets.

## Selection and safety

The tool discovers directories containing `.fingerprint/` without following symlinks:
`debug`, `release`, cross-target profiles and `llvm-cov-target` profiles all work.
Within each profile, `deps/`, `.fingerprint/` and `build/` entries named
`[lib]<name>-<16 hex>[.<ext>]` are grouped by unit name **and** metadata hash.
Unit names replace hyphens with underscores, as rustc names the compiled unit: Cargo’s
package spelling in `.fingerprint/` and `build/` thus matches `deps/` and `incremental/`.
Only library artifacts (`rlib`, `rmeta`, `so`, `dylib`, `a`) strip their `lib` prefix;
Cargo `.d` files and executable names retain a unit’s own leading `lib` (such as `libc`).
A build's date is the newest mtime of all its entries, including every nested file
and directory. For each unit, the newest build always survives, even if expired;
other builds survive if their newest mtime is within the stated window.

`incremental/<name>-<id>` directories use the same selection independently by name.
Selection is independent for every profile. `builds_removed` counts unit/hash builds;
incremental directories contribute to the sizes but not that count.

Profile-root files, `doc/`, `tmp/`, unrecognized output and unnamed target directories
are untouched. Symlinked profiles and containers are reported and never traversed.
Build-entry and nested symlinks contribute neither time nor allocated bytes; deleting
a selected build unlinks its symlinks without touching their referents. A read failure
while measuring a build protects that build. Removal failures are reported and the pass
continues. Run maintenance while the target is idle; filesystem permissions constrain
what it can delete.

Sizes count allocated filesystem blocks, as `du` does, rather than logical file length.
Hard-linked inodes are counted once among retained entries; removing a link frees no
blocks while another link survives, including a link outside the target.

No decision reads or depends on access time. `cargo-sweep --time` is unsuitable for
this policy because it judges access time: Cargo's own fingerprint reads refresh
atime under `relatime`, making superseded output appear recently used.

## Library

The crate imports as `cargo_prune_superseded`:

```rust
use std::{path::Path, time::SystemTime};
use cargo_prune_superseded::{Options, parse_span, prune_target};

let options = Options { keep_within: parse_span("2d")?, dry_run: true };
let report = prune_target(Path::new("target"), &options, SystemTime::now())?;
let _: std::path::PathBuf = report.path;
let _: u32 = report.profiles;
let _: (u64, u64, u64) = (report.builds_removed, report.bytes_freed, report.bytes_retained);
for entry in report.skipped {
    let _: (std::path::PathBuf, String) = (entry.path, entry.reason);
}
# Ok::<(), cargo_prune_superseded::Error>(())
```

`Options` holds `keep_within: Duration` and `dry_run: bool`. The returned `TargetReport`
holds a target `path: PathBuf`, `profiles: u32`, `builds_removed: u64`, allocated
`bytes_freed: u64` and `bytes_retained: u64`, and `skipped: Vec<Skipped>`.
Each `Skipped` contains its `path: PathBuf` and `reason: String`. Invalid spans and
unreadable targets return `Error`.

The caller supplies `now`. `prune_target_measured` also returns walk/removal timings
and entry counts for telemetry.

## onevcs pool maintenance

Each Rust repository installs its own pinned release and declares a `just` recipe:

```just
pool-maintain:
    cargo-prune-superseded --keep-within 2d target
```

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The fixed task requires this onevcs usage example and explicitly limits onevcs knowledge to README usage; a cross-product schema gate would expand that boundary. -->
Its `onevcs.toml` names that recipe:

```toml
[pool.maintain]
command = ["just", "pool-maintain"]
```
<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->

onevcs runs the repository's command in idle warm-pool slots; this tool knows only the
target directories it receives. When `ONEBUDGETSPEC_RESULT` names a file, the binary
uses `onebudgetspec-core::report` to record the whole invocation's wall-clock seconds,
with entries walked, profiles, builds removed, bytes freed, and walk/removal seconds.
Outside a budget check it writes no telemetry file. A telemetry write failure is
diagnosed on stderr; completed pruning passes still exit 0, and the budget runner
rejects the missing recording.

## Development

`just bootstrap` prepares a clean clone. `just check` runs the affected quality tier;
`just lint-llm-diff` judges the change. Ordinary e2e fixtures use real mtimes and
filesystem refusals without compiling fixture crates. `just workload` separately runs
the two-crate Cargo journey, an approximately 170,000-entry sparse fixture, and its
workload project’s pass-time budget. Only release pull requests run that recipe after the full gate.

Changes are in [CHANGELOG.md](https://github.com/nickderobertis/cargo-prune-superseded/blob/main/crates/cargo-prune-superseded/CHANGELOG.md). MIT licensed.
