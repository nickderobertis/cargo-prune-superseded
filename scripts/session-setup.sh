#!/usr/bin/env bash
# Session provisioner for Claude Code's SessionStart hook (.claude/settings.json): makes
# a fresh session able to run the `just` recipes. Ensures `just` (from PyPI's
# `rust-just` via uv, which needs no Rust toolchain), names any other missing tool
# `just bootstrap` needs, persists PATH for the session, then runs setup-llmlint.sh.
#
# No-op in CI, which provisions itself. Never blocks session startup: no `set -e`, and
# it always exits 0. Silent on success; each problem is logged with what to do.
# crates/dev-setup-e2e drives it.
set -uo pipefail

readonly JUST_MIN="1.51.0"
# The inherited PATH, so PATH is persisted only when BIN_DIR was not already on it.
readonly ORIG_PATH="${PATH}"

log() { printf 'session-setup: %s\n' "$*" >&2; }

if [ -z "${HOME:-}" ] || [ ! -d "$HOME" ]; then
  log "HOME is unset or not a directory, so nothing can be installed under it; set HOME, then re-run"
  # llmlint: ignore[cli_output_contract] a SessionStart hook must never fail session startup, so this refusal exits 0 by design after reporting on stderr; crates/dev-setup-e2e asserts both.
  exit 0
fi
readonly BIN_DIR="$HOME/.local/bin"

[ -z "${CI:-}" ] || exit 0

export PATH="${BIN_DIR}:${PATH}"

ensure_just() {
  command -v just >/dev/null 2>&1 && return 0
  if ! command -v uv >/dev/null 2>&1; then
    log "neither just nor uv is installed; install uv (https://docs.astral.sh/uv/), then re-run 'scripts/session-setup.sh'"
    return 0
  fi
  uv tool install --quiet --upgrade "rust-just>=$JUST_MIN" >&2 \
    || log "rust-just install failed (above); install just (https://just.systems) by hand"
}

verify_prereqs() {
  local tool
  for tool in cargo bun; do
    command -v "$tool" >/dev/null 2>&1 \
      || log "$tool is not on PATH; install it (cargo: https://rustup.rs, bun: https://bun.sh), then run 'just bootstrap'"
  done
}

persist_session_env() {
  [ -n "${CLAUDE_ENV_FILE:-}" ] || return 0
  case ":${ORIG_PATH}:" in
    *":${BIN_DIR}:"*) ;;
    *) if ! error="$( { printf 'export PATH=%q\n' "${BIN_DIR}:${PATH}" >> "$CLAUDE_ENV_FILE"; } 2>&1 )"; then
         log "could not write PATH to the session env file $CLAUDE_ENV_FILE (${error##*: }); check that it is writable, then re-run"
       fi ;;
  esac
}

ensure_just
verify_prereqs
persist_session_env

setup_llmlint="$(dirname "$0")/setup-llmlint.sh"
# setup-llmlint.sh reports its own problems and always exits 0, so its status is not read.
if [ -x "$setup_llmlint" ]; then
  "$setup_llmlint"
else
  log "$setup_llmlint is missing or not executable; restore it from git (chmod +x), then re-run"
fi
# llmlint: ignore[cli_output_contract] a SessionStart hook must never fail session startup, so this exits 0 by design; every problem above is reported on stderr with what to do, and the journeys in crates/dev-setup-e2e assert both.
exit 0
