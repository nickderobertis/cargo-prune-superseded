# AGENTS.md

Durable instructions for humans and agents working in this repo. Write for a future
maintainer, not as a session log; deterministic steps live in scripts and `just` recipes.

> Keep this file terse — it is always-loaded context. Add a line only if a future task
> needs it **and** wouldn't surface it anyway (a failing gate, `just --list`, the code).
> Folder-scoped rules go in that folder's `AGENTS.md`. `CLAUDE.md` is a symlink to this
> file; edit `AGENTS.md` only.

## What this repo is

A cargo subcommand and its library, published to crates.io for the Rust repositories that
install it ([README](README.md)). Its one job deletes files, so safety is the bar.

## Two standing goals on every task

The user's request is the priority, but carry two goals into every task, folding either in
when it is the lowest-error path and otherwise proposing it as a follow-up:

1. **Engineer the context for next time** — realistic e2e for what the user sees
   (especially a bug the suite missed), scripts for repeated steps, terse notes here.
2. **Engineer the codebase and environment** — clean, maintainable, reproducible: `just
   bootstrap` from a clean clone, strict gates, local/CI parity on one pinned toolchain.

## Stack and composition

<!-- llmlint: ignore-block[agents_md_durable_and_terse] create-repo requires this composition record in AGENTS.md (its baseline checker fails without the shape, languages, references and exclusions), and the exclusions are deliberate decisions no file shows. -->

- **Product shape:** cli, plus the library shape's rules (the crate's library is a public
  contract too).
- **Language(s):** rust
- **References composed:** base.md, project-graph.md, shapes/cli.md, shapes/library.md,
  languages/rust.md, intersections/rust-cli.md, ci.md, llmlint.md, releasing.md,
  tools/onebudgetspec.md.
- **Excluded, and why:**
  - GitHub Release archives, `install.sh`, `action.yml`: every user of a cargo
    subcommand has cargo, so crates.io is the one install surface.
  - A tag-triggered publish workflow: crates.io is the only artifact, so release-plz
    publishes it from the merged release PR (idempotent: it skips a published version).
  - A live tier: nothing contacts an external service but the release probe, whose
    crates.io read its tests stub.
  - Per-project budgets: no cost figure exists yet; the pruning work registers its own.
  - Nx's eslint boundary rule: no JS lint stack; `scripts/check-boundaries.mjs` enforces
    the tags.
  - asdf/direnv: `rust-toolchain.toml` and `package.json` pin the toolchain.
  - Windows in the gate: its scripts and tests are POSIX; the install job proves Windows.
  - The CI install job runs `cargo install --locked --path` on the `cargo package`
    archive rather than the README's registry command: until a release exists that
    archive is exactly what crates.io would serve.
  - Measured tier thresholds: no CI history yet; start at 10 min p95 (affected tier) and
    5 min (lint/unit), then re-derive from measurements.

<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->

## Command surface

Use the `just` recipes (`just --list`) rather than hand-rolling their commands; the
llmlint recipes sit outside the deterministic gate.

## Commits, releases, and merging

- **Squash-merge only, via PR, with auto-merge.** `main` is protected; the PR title is the
  squash subject and its description (the template's terse **What** / **Why**) the body.
  Queue with `gh pr merge --auto --squash`; head branches auto-delete; admins may bypass in
  a break-glass. Every gating check is required (`just governance --verify` reads them
  back); rename a CI job and the governance recipe moves with it.
- **Releases: release-plz behind a release PR.** Merging the release PR is the only human
  step in a release; nobody edits a version, tags or publishes by hand. Bump policy
  (pre-1.0): `feat` and breaking changes → minor; `fix`/`perf`/`refactor`/`build` →
  patch; anything else → no release. Post-1.0 a breaking change is a major.
- **Broader tier at release-prep:** releases are batched behind the release PR, so it
  alone gets the full sweep (`just check all`); every other PR and each squash commit on
  `main` get the affected tier, and publishing re-gates nothing.
- **Secrets** live in the repository's secret store, synced from Bitwarden by gh-secrets
  (`gh-secrets.json` names them, never values): `RELEASE_PLZ_TOKEN` (a PAT — a tag or PR
  made with the default token triggers no workflow), `CARGO_REGISTRY_TOKEN`, and
  `CLAUDE_CODE_OAUTH_TOKEN` for the llmlint check.

## Invariants (non-negotiable)

- A diagnostic is fixed, or suppressed at one site with a stated reason.
- **Tests are realistic, not mocked, and complete, not minimal.** Proof is proportionate
  to the change per the [create-repo base rules](https://github.com/nickderobertis/dero-skills/blob/main/skills/bootstrap/create-repo/assets/llmlint/base.llmlint.yml).
  The binary is driven as a subprocess (exit code, stdout, stderr); only network
  boundaries (crates.io, the package installers) are stubbed.
- Security is gate-level: no secrets in the tree; least-privilege grants everywhere.

## Tests are context engineering

The suite is the only QA loop: cover happy path **and** failure/recovery for every
journey; coverage is a floor, not the target. **A cost figure is a budget, never a test assertion; a wall-clock
bound never tells two behaviours apart** (wait on the event); time-feature contracts,
exact counts and product-enforced limits stay assertions ([when a requirement is a
budget](https://github.com/nickderobertis/dero-skills/blob/main/skills/bootstrap/create-repo/references/tools/onebudgetspec.md#when-is-a-requirement-a-budget)).

## Keeping the allowlist current

`.claude/settings.json` holds the agent allowlist. When a routine command joins the
workflow, add it there rather than re-approving it; keep it narrow.

## After the main task

Propose only materially helpful follow-ups — a script for a step done by hand, a note
here, a test that would have caught a bug — each with its likely impact. Skip busywork.
