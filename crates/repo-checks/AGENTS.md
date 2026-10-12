# repo-checks

- Test-only and offline: holds the release and CI wiring to one another, and drives the
  gate's own tooling scripts from scratch copies so a run never touches this checkout.
- Change that wiring or tooling and these checks together; a mutation of what a check
  guards must fail it.
