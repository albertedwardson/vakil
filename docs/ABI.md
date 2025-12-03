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

## ABI summary

Vakil plugins MUST be Rust dynamic libraries.
Vakil MUST NOT use a C ABI as the public plugin contract.
Vakil MUST load plugins only at startup.
Vakil MUST NOT watch, pull, or hot-reload plugins or configuration at runtime.
Vakil plugins MAY route, filter, inspect, reject, and mutate HTTP, TCP, and UDP traffic.
Vakil plugins MUST own their own configuration format and startup configuration parsing.

## Discovery and loading

The host discovers plugin libraries during startup using environment variables.

- `VAKIL_PLUGIN_PATHS` contains a list of paths to search.
- `VAKIL_PATH_SEP` contains the separator used to split `VAKIL_PLUGIN_PATHS`.
- If `VAKIL_PATH_SEP` is not set, the host MUST use `;` as the separator.

Each entry in `VAKIL_PLUGIN_PATHS` MAY refer to a plugin library file or a directory containing one or more plugin libraries.
The host MUST resolve the entries in order and load matching libraries before serving traffic.
If a path cannot be loaded, the host MUST treat that as a startup error unless the deployment policy explicitly marks it optional.

## ABI goals

The Vakil ABI is designed to:

- expose full control for routing decisions for HTTP, TCP, and UDP to the plugins,
- support traffic filtering and mutation,
- keep plugin-owned configuration outside the host config format,
- remain Rust-native and versioned,
- make ownership and threading rules explicit.

## Public types

The exact implementation may evolve, but the ABI is defined in terms of the following stable concepts.

```rust
pub struct PluginManifest {
    pub name: stabby::string::String,
    pub version: SemVer,
}

pub struct PluginInitContext {
    pub library_path: stabby::string::String,
    pub plugin_dir: stabby::option::Option<stabby::string::String>,
    pub host_version: SemVer,
    pub env: EnvSnapshot,
}

pub struct RouteContext {
    ...
}

pub struct HttpContext {
    ...
}

pub struct TcpContext {
    ...
}

pub struct UdpContext {
    ...
}
```

Vakil SHOULD use stabby's stable string, vector, option, and result types at the ABI boundary.
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

A conceptual Rust interface looks like this:

```rust
pub trait VakilPluginRoot {
    fn manifest(&self) -> PluginManifest;
    fn create_http(&self) -> Option<Box<dyn VakilHttpPlugin>>;
    fn create_tcp(&self) -> Option<Box<dyn VakilTcpPlugin>>;
    fn create_udp(&self) -> Option<Box<dyn VakilUdpPlugin>>;
}

pub trait VakilHttpPlugin {
    fn init(&mut self, ctx: &PluginInitContext) -> Result<(), PluginError>;
    fn on_route(&mut self, ctx: &mut RouteContext) -> Result<RouteDecision, PluginError>;
    fn on_request_headers(&mut self, ctx: &mut HttpContext) -> Result<HookOutcome, PluginError>;
    fn on_request_body(&mut self, ctx: &mut HttpContext) -> Result<HookOutcome, PluginError>;
    fn on_response_headers(&mut self, ctx: &mut HttpContext) -> Result<HookOutcome, PluginError>;
    fn on_response_body(&mut self, ctx: &mut HttpContext) -> Result<HookOutcome, PluginError>;
    fn on_trailers(&mut self, ctx: &mut HttpContext) -> Result<HookOutcome, PluginError>;
    fn on_local_reply(&mut self, ctx: &mut HttpContext) -> Result<HookOutcome, PluginError>;
    fn shutdown(&mut self);
}

pub trait VakilTcpPlugin {
    fn init(&mut self, ctx: &PluginInitContext) -> Result<(), PluginError>;
    fn on_route(&mut self, ctx: &mut RouteContext) -> Result<RouteDecision, PluginError>;
    fn on_connect(&mut self, ctx: &mut TcpContext) -> Result<HookOutcome, PluginError>;
    fn on_data(&mut self, ctx: &mut TcpContext) -> Result<HookOutcome, PluginError>;
    fn on_half_close(&mut self, ctx: &mut TcpContext) -> Result<HookOutcome, PluginError>;
    fn on_close(&mut self, ctx: &mut TcpContext) -> Result<HookOutcome, PluginError>;
    fn shutdown(&mut self);
}

pub trait VakilUdpPlugin {
    fn init(&mut self, ctx: &PluginInitContext) -> Result<(), PluginError>;
    fn on_route(&mut self, ctx: &mut RouteContext) -> Result<RouteDecision, PluginError>;
    fn on_datagram(&mut self, ctx: &mut UdpContext) -> Result<HookOutcome, PluginError>;
    fn on_session_start(&mut self, ctx: &mut UdpContext) -> Result<HookOutcome, PluginError>;
    fn on_session_end(&mut self, ctx: &mut UdpContext) -> Result<HookOutcome, PluginError>;
    fn shutdown(&mut self);
}
```

If a plugin supports multiple protocols, the host MAY instantiate more than one protocol trait from the same library.
If the plugin is shared, the host MAY call hooks from multiple threads and the plugin MUST synchronize its own mutable state.

## Host functions exposed to plugins

The host MUST expose as many functions required for a plugin to be as custumasible as possible.
The host MUST NOT provide runtime configuration watch or reload functions.
The host MUST NOT provide a generic config API that bypasses the plugin-owned configuration model.

## Manifest

The manifest describes the plugin to the host.
It MUST be available before any hooks are called.
The manifest SHOULD include:

- a stable plugin name,
- a semantic version or ABI version,
- a threading model,
- optional build metadata.

The host uses the manifest to decide whether the plugin is compatible with the current process, runtime, and policy.
If the manifest is incompatible, the host MUST reject the plugin during startup.

## Routing hooks

Routing is a first-class plugin capability.
Routing hooks allow the plugin to influence the selected upstream, rewrite the route, or reject the flow before forwarding begins.

A route decision MAY:

- keep the selected upstream,
- replace the selected upstream,
- change retry or timeout policy,
- block the flow,
- synthesize a response or error,
- annotate the flow for later hooks.

Routing hooks apply to HTTP, TCP, and UDP.
A routing hook MAY inspect listener metadata, peer metadata, SNI, headers, datagram metadata, and other flow attributes.

### Upstream selection policy

Vakil delegates upstream selection authority to plugins: a plugin MAY choose,
rewrite, or replace the upstream for a flow. The host does not impose runtime
policy that prevents a plugin from selecting an upstream. Deployments that
require stricter control SHOULD use external deployment policies (firewalls,
network namespaces, or sidecar guards) or configure the host to restrict
allowed upstreams at startup. The ABI and host do not validate or block
plugin-chosen upstreams beyond basic syntactic checks.

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
When a body hook is invoked for a chunk, `HttpMessage.body` MUST contain that chunk rather than a fully buffered message body.
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

This means the host does not parse plugin config and does not impose JSON, TOML, or YAML.

## Threading model

The manifest MUST declare the threading model.
The following threading models are expected:

- shared, internally synchronized,
- single-threaded,
- worker-local.

The host MAY execute hooks concurrently only when the manifest says that is safe.
A plugin that is not thread-safe MUST NOT be invoked concurrently by the host.

## Errors

Hook results SHOULD use a stable Rust error type or a compact status enum with an optional detail string.
A plugin error MAY indicate:

- invalid input,
- unsupported protocol or phase,
- temporary failure,
- permanent failure,
- policy rejection,
- internal plugin error.

The host MUST be able to distinguish success from rejection and from transport failure.

## Failure semantics

Vakil treats plugin execution failures as host-level policy: if any plugin in a
protocol chain returns an error for a given flow, the host MUST treat the
entire plugin chain for that flow as failed. The host SHOULD then apply the
failure policy documented elsewhere (for example, abort the flow, generate a
local-reply, or fall back to a safe upstream) as configured by deployment
policy. Hosts MUST NOT silently ignore plugin errors and continue as if the
plugin had not run.

Panics: Vakil does not attempt to recover from plugin panics. If a plugin
panics while executing a hook, the panic is permitted to propagate according
to the host process behaviour (which may abort). Plugin authors MUST avoid
panics in production code; the host MAY run plugins in isolated processes or
use OS-level supervision in deployments that require process-level isolation.

Host action on plugin-chain failure: When the host considers a plugin chain
to have failed for a flow (for example, due to a plugin returning an error),
the host SHOULD synthesize a local reply for the downstream peer. If any
plugin has populated the `HttpContext.response` during a request-phase hook,
the host MUST use that `HttpMessage` (status, headers, body, trailers) as the local
reply. If no plugin-supplied response is available, the host SHOULD fall back
to a compile-time constant default local-reply (for example, `DEFAULT_LOCAL_REPLY_STATUS = 502`).
Plugins may override this by populating `HttpContext.response`. This
ensures plugins can fully control local-replies while the host provides a
predictable fallback when necessary.


## Versioning and compatibility

The ABI MUST be versioned.
The host MUST reject plugins with incompatible ABI versions at load time.
Adding new hooks SHOULD be additive.
Changing ownership rules for existing fields MUST be treated as a breaking change.
Changing the meaning of a capability bit MUST be treated as a breaking change.

## Relationship to the current crates

The workspace currently contains a loader in [crates/plugin-host](../crates/plugin-host/src/lib.rs) that validates plugin manifests, captures startup metadata, and instantiates HTTP/TCP/UDP protocol modules.
The ABI-facing crates in [crates/plugin-api](../crates/plugin-api/src/lib.rs) and [crates/plugin-sys](../crates/plugin-sys/src/lib.rs) now model the richer context data described above and should continue to evolve with the document.
