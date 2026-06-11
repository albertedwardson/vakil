# Vakil Native Plugin ABI v0.1.0

This document specifies the native plugin ABI for Vakil.
Vakil plugins are Rust dynamic libraries that are discovered and loaded only at startup.

## Terminology

- *Plugin* - a Rust dynamic library that implements the Vakil plugin ABI.
- *Host* - the Vakil proxy process that discovers, loads, and executes plugins.
- *Root module* - the exported library entry point used to construct plugin instances.
- *Plugin instance* - a loaded plugin object that handles routing and traffic hooks.
- *Flow* - one logical traffic unit, such as an HTTP request, TCP connection, or UDP session.
- *Hook* - a callback invoked by the host on a plugin instance.
- *Hook phase* - the point in a flow lifetime at which a hook runs.
- *Mutable view* - a host-owned buffer or structure exposed to a plugin for in-place mutation.
- *Replacement value* - a new owned value returned by a plugin when in-place mutation is not sufficient.
- *Plugin-owned config* - configuration data interpreted by the plugin itself; the host does not define a shared config schema.

## Summary

Vakil plugins are a Rust dynamic libraries.
Vakil doesn't use a C ABI natively as the public plugin contract.
Vakil loads plugins only at startup.
Vakil doesn't watch, pull, or hot-reload plugins or configuration at runtime.
Vakil plugins MAY route, filter, inspect, reject, and mutate HTTP, TCP, and UDP traffic.
Vakil plugins MUST own their own configuration format and startup configuration parsing.

## Discovery and loading

The host discovers plugin libraries during startup using environment variables.

- `VAKIL_PLUGIN_PATHS` contains a list of paths to search.
- `VAKIL_PATH_SEP` contains the separator used to split `VAKIL_PLUGIN_PATHS`.
- If `VAKIL_PATH_SEP` is not set, the host MUST use `;` as the separator.

Each entry in `VAKIL_PLUGIN_PATHS` MAY refer to a plugin library file or a directory containing one or more plugin libraries.
The host resolves the entries in order and load matching libraries before serving traffic.
If a path cannot be loaded, the host treats that as a startup error.

## Public types

The exact implementation may evolve, but the ABI is defined in terms of the following stable concepts.

See current implementation [here](../crates/vakil-plugin-api).

Vakil uses stabby's types at the ABI boundary.
The host and the plugin MUST agree on ownership for every value that crosses the ABI boundary.

The current implementation passes context values across callback boundaries as raw pointers to keep the exported function signatures ABI-stable while the data structures themselves remain `stabby`-checked.
HTTP request/response messages carry headers, bodies, trailers, and status so plugin callbacks can observe richer phase data and synthesize local replies without overloading path fields.

## Plugin lifecycle

A plugin library follows this lifecycle:

1. the host loads the library,
2. the host reads the manifest,
3. the host constructs the protocol plugins declared by the library,
4. the host calls `init` on each protocol plugin that the library exposes,
5. the host dispatches protocol-specific hooks,
6. the host calls `shutdown` on each protocol plugin before unloading the library or terminating.

There is no runtime reload phase.
A plugin change or configuration change requires a restart or redeploy.

## Entry points exposed by the plugin

A Vakil plugin library MUST export a root module that the host can load with `stabby`-style dynamic library loading.
The root module MUST provide the plugin manifest and factory methods for the protocol-specific plugin traits that the library supports.
Protocol factories that are not supported MAY be exported as `None` in the root module, so plugin authors do not need to provide no-op callbacks for unsupported protocols.

Current implementation of the interface located [here](../crates/vakil-plugin-sys/src/lib.rs).

If a plugin supports multiple protocols, the host will instantiate more than one protocol trait from the same library.
If the plugin is shared, the host MAY call hooks from multiple threads and the plugin MUST synchronize its own mutable state.

## Manifest

The manifest describes the plugin to the host.
It MUST be available before any hooks are called.
The manifest SHOULD include:

- a stable plugin name,
- a semantic version or ABI version,
- optional build metadata.

The host uses the manifest to decide whether the plugin is compatible with the current runtime.
If the manifest is incompatible, the host rejects the plugin during startup.

## Routing hooks

Routing is a first-class plugin capability.
Routing hooks allow the plugin to influence the selected upstream, rewrite the route, or reject the flow before forwarding begins.

A route decision MAY:

- keep the selected upstream,
- replace the selected upstream,
- change retry or timeout policy,
- synthesize a response or error,
- annotate the flow for later hooks.

Routing hooks apply to HTTP, TCP, and UDP.
A routing hook MAY inspect listener metadata, peer metadata, SNI, headers, datagram metadata, and other flow attributes.

### Upstream selection policy

Vakil delegates upstream selection authority to plugins: a plugin MUST choose, rewrite, or replace the upstream for a flow.
Vakil doesn't validate or block plugin-chosen upstreams beyond basic syntactic checks.

## HTTP hooks

HTTP hooks are divided by phase.
A plugin MAY implement one or more of the following phases:

- request headers,
- request body,
- response headers,
- response body,
- trailers,
- synthesise own reply,
- redirect.

HTTP hooks MAY inspect and mutate headers and bodies.
Body hooks MUST be invoked once per body chunk so large request and response payloads can be processed incrementally.
When a body hook is invoked for a chunk, `HttpRequest/HttpResponse.body` MUST contain that chunk rather than a fully buffered message body.
HTTP hooks MAY short-circuit the request with a local response or reject the request.
HTTP hooks MUST preserve ownership rules when replacing a buffer or message.

## TCP hooks

TCP hooks are divided by phase.
A plugin MAY implement one or more of the following phases:

- connect or accept,
- stream data,
- half-close,
- full-close.

TCP hooks MAY inspect and mutate stream bytes.
TCP hooks MAY replace upstream selection or block the connection.
TCP hooks MAY treat payloads as opaque bytes unless the plugin is protocol-aware.

## UDP hooks

UDP hooks are divided by phase.
A plugin MAY implement one or more of the following phases:

- datagram receive,
- datagram send,
- session start,
- session end.

UDP hooks MAY inspect and mutate datagrams.
UDP hooks MAY reject, drop, or reroute a datagram.
UDP hooks MAY preserve packet boundaries unless a replacement value explicitly changes them.

## Mutation model

Vakil prefers mutable views over copies.
The host MAY pass a plugin a mutable buffer or mutable message view when in-place editing is allowed by the transport and by ownership rules.
The plugin MAY:

- modify the buffer in place,
- replace the current buffer with a new owned value,
- drop the current payload,
- short-circuit the flow with a synthesized result.

If a mutation would exceed the available space or violate transport constraints, the plugin MUST return a replacement value instead of forcing unsafe mutation.
The host is responsible for applying the final buffer semantics to the underlying transport.

## Plugin-owned configuration

Vakil does not define a shared configuration file format.
Each plugin defines its own configuration model.
A plugin MAY read files, directories, environment variables, or other startup-only resources that it owns.
The host only provides discovery metadata and the startup context needed to locate those resources.

## Errors

Hook results MUST use provided error type with an optional detail string.
The host MUST be able to distinguish success from rejection and from transport failure.

Vakil treats plugin execution failures as host-level policy: if any plugin in a protocol chain returns an error for a given flow, the host will treat the entire plugin chain for that flow as failed.

Vakil does not attempt to recover from plugin panics. If a plugin panics while executing a hook, the panic is permitted to propagate according to the host process behaviour (which may abort).

When the host considers a plugin chain to have failed for a flow (for example, due to a plugin returning an error), the host MAY synthesize a local reply for the downstream peer. If any plugin has populated the `HttpContext.response` during a request-phase hook, the host will use that `HttpMessage` (status, headers, body, trailers) as the local reply. If no plugin-supplied response is available, the host will fall back to a default local-reply.
