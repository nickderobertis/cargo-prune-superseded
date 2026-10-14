# cargo-prune-superseded-e2e

- Test-only (`publish = false`): journeys spawn the built binary and assert exit code,
  stdout and stderr. Never call the library in-process here.
- Every journey covers its failure path too; keep them offline and tempdir-isolated.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] The fixed task explicitly requires AGENTS.md journey lists; this scoped inventory fulfills that documentation contract without copying fixture setup or commands. -->
- Journeys cover mtime retention with fresh atimes, allocated blocks and hard links,
  per-profile/unit isolation, nested mtimes, symlink escapes, CLI/JSON/human output,
  dry-run, telemetry and real permission failures with continued processing.

<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
