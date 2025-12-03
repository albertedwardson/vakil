# Väkil - Pluggable Reverse-Proxy & Traffic Filtering System

Project goals:

- sane, usable API
- programmable
- broad extensibility, customization
- high performance, low overhead
- proxy UDP, TCP, HTTP(S)

Non-goals:

- no proxying and routing logic in core
- not cross-platform
- not replace existing solutions
- no multi-language plugins
- no other protocols

## Plugin-first policy

- **Plugins own policy**: routing, filtering, and local-reply decisions are owned by plugins. The host provides API and execution environment only.
- **Upstream selection**: plugins may replace the upstream with structured parameters (scheme, host, port, SNI); the host performs only basic validation and relies on deployment policies for network restrictions.
- **Mutation model**: plugins may mutate host-provided views in-place where allowed by transport and ownership rules.
- **Failure semantics**: if any plugin in a chain returns an error, the entire plugin chain is considered failed and the host synthesizes a local-reply (preferring plugin-provided response). Panics are not caught by the host and may propagate.
- **Plugin config**: plugins parse their own configuration using `PluginInitContext` (host provides `plugin_dir` and `env` snapshot at startup).
