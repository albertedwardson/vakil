tests_common — shared integration test helpers

This crate provides reusable helpers and fixtures for integration and end-to-end
tests in this workspace.

Usage

- Add a path dependency on `tests_common` from your test crate's `Cargo.toml`:

  tests_common = { path = "../tests-common" }

- In your test source import the helpers you need:

  use tests_common::{reserve_port, wait_for_port, spawn_backend, spawn_proxy, proxy_request, RecordingHooks, HttpPeer};

- Run tests as usual with cargo:

```bash
cargo test -p e2e-tests --tests
```

Notes

- `tests_common` is intended only for test code and offers convenience wrappers
  around spawning a Pingora server, a simple TCP backend, and recording hooks.
- For runtime startup coverage, prefer subprocess e2e tests that boot `vakil-cli`
  with a real config file and then drive live TCP/UDP traffic against the child.
- Keep helpers stable: prefer adding new helpers here when multiple tests
  require the same setup.
