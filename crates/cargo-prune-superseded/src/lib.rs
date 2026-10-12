//! `cargo-prune-superseded`: prune superseded cargo build output by modification time.
//!
//! This release is a placeholder: it parses its command line — `--help` and `--version` —
//! and prunes nothing. The library's public surface is only what the binary needs.

use clap::Parser;

/// The command line the `cargo-prune-superseded` binary accepts.
#[derive(Debug, Parser)]
#[command(name = "cargo-prune-superseded", version, about, long_about = None)]
pub struct Cli {}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Cli;

    #[test]
    fn command_definition_is_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn version_is_the_crate_version() {
        assert_eq!(
            Cli::command().get_version(),
            Some(env!("CARGO_PKG_VERSION"))
        );
    }
}
