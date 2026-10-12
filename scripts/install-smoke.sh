#!/usr/bin/env bash
# The end-user install path, from the exact archive crates.io would serve: package the
# crate as the working tree stands (`cargo package` verifies it builds from the archive
# alone), install it locked from that archive into an empty root outside the clone, and
# run the installed binary from that root. Until a release exists, that archive stands in for the README's
# `cargo install --locked cargo-prune-superseded`. Offline: it builds from the crates
# `cargo fetch --locked` (in `just bootstrap`) already cached, so it contacts no registry.
# Quiet on success; on failure the step, its output and what to do. repo-checks drives
# each failure.
set -euo pipefail

root_dir="$(dirname "${BASH_SOURCE[0]}")/.."
cd "$root_dir" || { echo "install-smoke: cannot enter $root_dir; run it from a checkout of the repository." >&2; exit 1; }

readonly CRATE="cargo-prune-superseded"

fail() {
  [ -z "$3" ] || printf '%s\n' "$3" >&2
  echo "install-smoke: $1" >&2
  echo "install-smoke: $2" >&2
  exit 1
}

version="$(cargo pkgid -p "$CRATE" 2>&1)" \
  || fail "cannot read $CRATE's version (above)." "run 'cargo pkgid -p $CRATE' from the repository root and fix what it reports." "$version"
version="${version##*[#@]}"
log="$(cargo package --locked --offline --allow-dirty -p "$CRATE" 2>&1)" \
  || fail "'cargo package' failed (above): the crate does not build from its published archive." "run 'cargo fetch --locked' if a crate is missing from the cache, or add any file the build needs to the package (Cargo.toml include/exclude), then re-run 'just install-smoke'." "$log"

install_root="$(mktemp -d 2>&1)" \
  || fail "cannot create a temporary install root (above)." "free space or fix TMPDIR, then re-run 'just install-smoke'." "$install_root"
trap 'rm -rf "$install_root" || echo "install-smoke: remove $install_root by hand." >&2' EXIT
log="$(cargo install --locked --offline --root "$install_root" --path "target/package/$CRATE-$version" 2>&1)" \
  || fail "installing the packaged crate failed (above)." "fix the build error it names (Cargo.lock must be committed and current), then re-run 'just install-smoke'." "$log"

cd "$install_root" || fail "cannot enter the install root $install_root." "check TMPDIR is accessible, then re-run 'just install-smoke'." ""
out="$("./bin/$CRATE" --version 2>&1)" \
  || fail "the installed binary's --version exited non-zero (above)." "run the binary's --version from a clone (cargo run -p $CRATE -- --version) and fix it." "$out"
[ "$out" = "$CRATE $version" ] \
  || fail "--version printed '$out', expected '$CRATE $version'." "make the command's name and version come from the crate manifest (clap's name and version)." ""
out="$("./bin/$CRATE" --help 2>&1)" \
  || fail "the installed binary's --help exited non-zero (above)." "run the binary's --help from a clone (cargo run -p $CRATE -- --help) and fix it." "$out"
case "$out" in
  *"Usage: $CRATE"*) ;;
  *) fail "--help printed no usage naming $CRATE (above)." "keep the clap command's name '$CRATE' so --help names it." "$out" ;;
esac
