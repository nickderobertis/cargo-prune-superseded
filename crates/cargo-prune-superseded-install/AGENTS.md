# cargo-prune-superseded-install

- `smoke.sh` is the end-user install path: keep it the path a user takes (`cargo install
  --locked`, then the installed binary, run from outside the clone). Its journeys drive
  each verdict; the `test` target also runs it for real.
