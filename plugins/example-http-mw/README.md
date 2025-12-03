# example-http-mw

HTTP-only example plugin used by subprocess e2e coverage.

## Behavior

- Keeps route decision as-is.
- Logs request/response phases.
- Returns a local reply for `/local-reply`.
- Logs request body and response body chunks when present.

## Test

```bash
cargo test -p example-http-mw
```
