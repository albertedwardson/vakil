# example-http-url-filter

HTTP example plugin that rejects requests whose request target cannot be parsed as a WHATWG URL by Ada.

## Behavior

- Reconstructs the request target from the HTTP authority and path.
- Uses `ada-url` to validate the target against the WHATWG URL parser.
- Synthesizes a `400 Bad Request` local reply when parsing fails.

## Test

```bash
cargo test -p example-http-url-filter
```
