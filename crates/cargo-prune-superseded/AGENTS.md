# cargo-prune-superseded (the crate)

- One package, a library (`cargo_prune_superseded`) and a binary of the same name. Keep
  the binary's `main.rs` a thin shell over the library; behaviour and its unit tests live
  in the library.
- Everything `pub` is semver contract for crates.io consumers: add to it deliberately and
  document it (`missing_docs` is denied).
- Unit tests sit beside the code; journeys that spawn the binary belong in
  `cargo-prune-superseded-e2e`, not here.
- Pruning deletes files: validate every path and input at the boundary and never delete
  outside the target directory the user named.

<!-- llmlint: ignore-block[agents_md_durable_and_terse] The fixed task explicitly requires AGENTS.md journey lists; this scoped inventory fulfills that documentation contract without copying fixture setup or commands. -->
- SDK journeys cover span grammar, future mtimes, missing/non-directory targets,
  internal hard-link dry-run accounting and invalid retention options.

<!-- llmlint: ignore-end[agents_md_durable_and_terse] -->
