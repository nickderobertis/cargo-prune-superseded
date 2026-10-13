# Shared pruning fixtures

- Test support only, unpublished. Consumers declare dev-dependencies and Nx edges;
  keep production behavior out of this crate.
- Helpers create real Cargo layouts and mtimes; inventories preserve bytes and types
  without following symlinks. The ordinary journeys compile no fixture crates.
