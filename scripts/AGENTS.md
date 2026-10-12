# scripts

- Quiet on success; on failure print the error and the concrete next action.
- `release-probe.sh` is a contract consumers (onevcs) spawn: three answers only — a
  version on stdout, empty stdout (none published), or a non-zero exit with the reason
  (not answered).
- `session-setup.sh` / `setup-llmlint.sh` always exit 0: a flaky install must never block
  session startup.
