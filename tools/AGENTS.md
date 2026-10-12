# tools

- The gate's own tooling. Quiet on success; on failure the error and the next action.
- Every Rust crate's `test` target runs through `rust-coverage.sh`, so each depends on
  this project; repo-checks drives each script from a scratch copy.
