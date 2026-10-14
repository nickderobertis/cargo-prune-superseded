# Release workloads

- `just workload` runs the real two-crate Cargo journey and the sparse-target telemetry
  journey, then this project's budget. No ordinary gate target depends on `workload`.
- Keep recordings under `dist/prune-workload/telemetry`; elapsed time belongs to the
  budget, never to an assertion. The analysis script reads telemetry and calls the SDK.
