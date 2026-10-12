# cargo-prune-superseded

Prune superseded cargo build output by modification time.

> **Status:** placeholder release. The binary answers `--help` and `--version` and prunes
> nothing yet.

## Install

```sh
cargo install --locked cargo-prune-superseded
```

Supported on Linux, macOS and Windows.

## Usage

```sh
cargo-prune-superseded --help
cargo-prune-superseded --version
```

Exit codes: `0` on success; `2` for a command-line usage error (an unknown flag, say),
with the error on stderr.

### As a library

The crate's library, `cargo_prune_superseded`, exposes the command line the binary
parses:

```rust
use cargo_prune_superseded::Cli;
use clap::Parser;

let cli = Cli::parse_from(["cargo-prune-superseded"]);
```

Changes are listed in [CHANGELOG.md](crates/cargo-prune-superseded/CHANGELOG.md).

## Development

`just bootstrap` sets up a clone; `just check` is the gate.

## License

MIT
