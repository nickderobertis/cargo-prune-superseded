# cargo-prune-superseded-e2e

- Test-only (`publish = false`): journeys spawn the built binary and assert exit code,
  stdout and stderr. Never call the library in-process here.
- Every journey covers its failure path too; keep them offline and tempdir-isolated.
