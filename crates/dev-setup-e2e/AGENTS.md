# dev-setup-e2e

- Test-only (`publish = false`): runs `scripts/bootstrap.sh` and the session provisioners
  as subprocesses in a sandboxed HOME and PATH. Only network installers are stubbed.
- The provisioners stay silent on success and never exit non-zero; journeys assert both.
