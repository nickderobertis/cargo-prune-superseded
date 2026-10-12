//! The `cargo-prune-superseded` binary.

use clap::Parser;

fn main() {
    cargo_prune_superseded::Cli::parse();
}
