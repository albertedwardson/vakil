# example-l4-sticky

Combined TCP + UDP example plugin that keeps a sticky backend assignment per client IP and listener.

## Behavior

- Uses `peer` as the downstream socket address, but only the client IP participates in the sticky key.
- Shares one backend choice across TCP and UDP for the same client IP + listener.
- Uses `gxhash` for fast non-cryptographic hashing.
- Keeps mappings in memory with a TTL.
- Reads startup config from `VAKIL_L4_STICKY_CONFIG` or falls back to `sticky-l4.toml` in the plugin directory.

## Config

```toml
ttl_secs = 900
fallback_backends = ["127.0.0.1:9001", "127.0.0.1:9002"]

[[listeners]]
name = "listener-a"
backends = ["127.0.0.1:9101", "127.0.0.1:9102"]
```

## Build

```bash
cargo test -p example-l4-sticky
```
