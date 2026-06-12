//! Stable ABI primitives for the proxy runtime and plugins.
//!
//! Design goals:
//! - Stable ABI via `stabby`
//! - Strong typing over stringly-typed metadata
//! - Separation of:
//!   - connection events
//!   - stream events
//!   - packet events
//!   - protocol events
//! - Runtime-owned transport orchestration
//! - Plugin-owned routing/modification logic
//!
//! This file intentionally avoids:
//! - generic metadata bags
//! - string-based protocol flags
//! - protocol conflation
//! - packet/stream confusion
//!
//! Philosophy:
//! The core owns IO, buffering, scheduling, timers, and protocol parsing.
//! Plugins own decisions (SansIO like).

use stabby::option::Option;
use stabby::string::String;
use stabby::vec::Vec;
use std::net::ToSocketAddrs;

macro_rules! vec_deref_impl {
    ($name:ident, $inner:ty) => {
        impl std::ops::Deref for $name {
            type Target = Vec<$inner>;

            #[inline]
            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl std::ops::DerefMut for $name {
            #[inline]
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

/// Stable unique identifier.
#[stabby::stabby]
#[derive(Clone, Copy, Debug)]
pub struct ID(pub u64);
impl ID {
    fn new() -> Self {
        Self(rand::random())
    }
}

/// Generic byte buffer.
#[stabby::stabby]
#[derive(Clone, Debug, Default)]
pub struct Bytes(pub Vec<u8>);
impl Bytes {
    fn new() -> Self {
        Self(Vec::new())
    }
}
impl<T> From<&T> for Bytes
where
    T: AsRef<[u8]> + ?Sized,
{
    fn from(value: &T) -> Self {
        Self(value.as_ref().into())
    }
}
impl<T> From<&mut T> for Bytes
where
    T: AsRef<[u8]> + ?Sized,
{
    fn from(value: &mut T) -> Self {
        Self(value.as_ref().into())
    }
}
vec_deref_impl!(Bytes, u8);

#[stabby::stabby]
#[derive(Clone, Copy, Debug)]
pub struct HttpStatus(pub u16);

#[stabby::stabby]
#[derive(Clone, Copy, Debug)]
pub struct SemVer {
    /// Major version for incompatible ABI changes.
    pub major: u8,
    /// Minor version for backward-compatible ABI additions.
    pub minor: u8,
    /// Patch version for fixes without ABI surface changes.
    pub patch: u8,
}
impl SemVer {
    pub fn from_string(input: std::string::String) -> Self {
        let mut parts = input.split('.').map(|s| s.parse().unwrap());
        Self {
            major: parts.next().unwrap(),
            minor: parts.next().unwrap(),
            patch: parts.next().unwrap(),
        }
    }
}

#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct KVPair {
    pub name: String,
    pub value: String,
}

#[stabby::stabby]
#[derive(Clone, Debug, Default)]
pub struct Headers(pub Vec<KVPair>);
impl Headers {
    fn new() -> Self {
        Self(Vec::new())
    }
}
vec_deref_impl!(Headers, KVPair);

#[cfg(feature = "pingora")]
impl<T> From<T> for Headers
where
    T: std::borrow::Borrow<pingora_http::RequestHeader>,
{
    fn from(req: T) -> Self {
        let req = req.borrow();
        let mut headers = Self::new();

        for (name, value) in req.headers.iter() {
            headers.0.push(KVPair {
                name: String::from(name.as_str()),
                value: String::from(value.to_str().unwrap_or("")),
            });
        }

        headers
    }
}
/// Environment snapshot (simple key/value list).
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct EnvSnapshot {
    pub entries: Vec<KVPair>,
}

/// Plugin manifest describing capabilities and metadata.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct PluginManifest {
    /// Stable plugin name.
    pub name: String,
    /// Plugin version.
    pub version: SemVer,
}

/// Plugin init context provided at startup.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct PluginInitContext {
    /// Absolute plugin library path.
    pub library_path: String,
    /// Optional resolved plugin directory.
    pub plugin_dir: Option<String>,
    /// Runtime version info.
    pub host_version: SemVer,
    /// Startup environment snapshot.
    pub env: EnvSnapshot,
}

/// Plugin error object.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct PluginError {
    pub message: Option<String>,
}

/// Socket endpoint.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct SocketAddress {
    pub host: String,
    pub port: u16,
}
impl From<&std::net::SocketAddr> for SocketAddress {
    fn from(addr: &std::net::SocketAddr) -> Self {
        Self {
            host: addr.ip().to_string().into(),
            port: addr.port(),
        }
    }
}

impl From<std::net::SocketAddr> for SocketAddress {
    fn from(addr: std::net::SocketAddr) -> Self {
        (&addr).into()
    }
}
impl From<SocketAddress> for std::net::SocketAddr {
    fn from(addr: SocketAddress) -> Self {
        (&addr).into()
    }
}
impl From<&SocketAddress> for std::net::SocketAddr {
    fn from(value: &SocketAddress) -> Self {
        std::net::SocketAddr::new(
            value
                .host
                .parse()
                .unwrap_or(std::net::Ipv4Addr::new(127, 0, 0, 1).into()),
            value.port,
        )
    }
}
impl From<SocketAddress> for std::string::String {
    fn from(value: SocketAddress) -> Self {
        format!(
            "{}:{}",
            <String as Into<std::string::String>>::into(value.host),
            value.port
        )
    }
}

#[cfg(feature = "pingora")]
impl From<&pingora_core::protocols::l4::socket::SocketAddr> for SocketAddress {
    fn from(addr: &pingora_core::protocols::l4::socket::SocketAddr) -> Self {
        addr.to_socket_addrs()
            .expect("uds is not supported")
            .next()
            .unwrap()
            .into()
    }
}

#[cfg(feature = "pingora")]
impl From<pingora_core::protocols::l4::socket::SocketAddr> for SocketAddress {
    fn from(addr: pingora_core::protocols::l4::socket::SocketAddr) -> Self {
        (&addr).into()
    }
}

/// Transport protocol.
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum TransportProtocol {
    Tcp = 0,
    Udp = 1,
}

/// Transport connection metadata.
///
/// Represents a logical transport connection.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct ConnectionInfo {
    /// Runtime-generated ID.
    pub id: ID,
    /// Local bound address.
    pub local_addr: SocketAddress,
    /// Peer/client address.
    pub peer_addr: Option<SocketAddress>,
    /// Transport protocol.
    pub protocol: TransportProtocol,
}

#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum FlowDirection {
    Inbound = 0,
    Outbound = 1,
}

#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct TransportContext {
    pub connection: ConnectionInfo,
    pub direction: Option<FlowDirection>,
    pub route: RouteDecision,
}

/// HTTP protocol version.
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Debug)]
pub enum HttpVersion {
    Http10 = 0,
    Http11 = 1,
    Http2 = 2,
    Http3 = 3,
}
#[cfg(feature = "pingora")]
impl From<pingora_http::Version> for HttpVersion {
    fn from(version: pingora_http::Version) -> HttpVersion {
        match version {
            pingora_http::Version::HTTP_09 => HttpVersion::Http10,
            pingora_http::Version::HTTP_10 => HttpVersion::Http10,
            pingora_http::Version::HTTP_11 => HttpVersion::Http11,
            pingora_http::Version::HTTP_2 => HttpVersion::Http2,
            pingora_http::Version::HTTP_3 => HttpVersion::Http3,
            _ => HttpVersion::Http11,
        }
    }
}

/// HTTP request.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub stream_id: ID,
    pub version: HttpVersion,
    pub is_tls: bool,
    pub method: String,
    pub authority: String,
    pub path: String,
    pub headers: Headers,
    pub body: Bytes,
}
#[cfg(feature = "pingora")]
impl<T> From<T> for HttpRequest
where
    T: std::borrow::Borrow<pingora_http::RequestHeader>,
{
    fn from(req: T) -> HttpRequest {
        let req = req.borrow();

        let method: String = req.method.as_str().into();
        let authority: String = req.uri.authority().map_or("", |v| v.as_str()).into();
        let path: String = req.uri.path().into();
        let version: HttpVersion = req.version.into();
        let headers: Headers = req.into();
        // RequestHeader does not contain a body; use empty Bytes.
        let body: Bytes = Bytes::new();
        HttpRequest {
            stream_id: ID::new(),
            version,
            is_tls: req.uri.scheme_str() == Some("https"),
            method,
            authority,
            path,
            headers,
            body,
        }
    }
}

/// HTTP response.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub stream_id: ID,
    pub version: HttpVersion,
    pub status: HttpStatus,
    pub headers: Headers,
    pub body: Bytes,
}

/// Routing action.
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Default, Debug)]
pub enum RouteAction {
    /// Keep existing upstream.
    #[default]
    Keep = 0,
    /// Replace upstream target.
    ReplaceUpstream = 1,
    /// Reject request/connection.
    Reject = 2,
    /// Return synthetic response.
    SyntheticResponse = 3,
}

/// Routing metadata passed from host runtime into plugin route hooks.
#[stabby::stabby]
#[derive(Clone, Default, Debug)]
pub struct RouteDecision {
    /// Selected action.
    pub action: RouteAction,
    /// Optional replacement upstream.
    pub upstream_to_set: Option<SocketAddress>,
    /// Http path, ignored in TCP and UDP contexts
    pub http_path: Option<String>,
}

/// HTTP request/response view passed to HTTP plugin callbacks.
#[stabby::stabby]
#[derive(Default, Clone, Debug)]
pub struct HttpContext {
    pub request: Option<HttpRequest>,
    pub response: Option<HttpResponse>,
    pub transport: Option<TransportContext>,
}
#[cfg(feature = "pingora")]
impl<T> From<T> for HttpContext
where
    T: std::borrow::Borrow<pingora_proxy::Session>,
{
    fn from(session: T) -> Self {
        let session = session.borrow();
        let req = session.req_header();

        let transport = TransportContext {
            connection: ConnectionInfo {
                id: ID::new(),
                local_addr: session.server_addr().map(SocketAddress::from).unwrap_or(
                    SocketAddress {
                        host: "".into(),
                        port: 0,
                    },
                ),
                peer_addr: session.client_addr().map(SocketAddress::from).into(),
                protocol: TransportProtocol::Tcp,
            },
            direction: Some(FlowDirection::Inbound).into(),
            route: RouteDecision::default(),
        };

        Self {
            request: Some(HttpRequest::from(req)).into(),
            response: Default::default(),
            transport: Some(transport).into(),
        }
    }
}
impl HttpContext {
    pub fn route(&self) -> Option<RouteDecision> {
        self.transport.as_ref().map(|t| t.route.clone()).into()
    }

    pub fn set_route(&mut self, route: RouteDecision) -> bool {
        let Some(mut transport) = self.transport.as_mut() else {
            return false;
        };

        transport.route = route;
        true
    }
}

/// TCP hook context.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct TCPContext {
    pub chunk: Option<Bytes>,
    pub meta: TransportContext,
}

/// UDP hook context.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct UDPContext {
    pub datagram: Option<Bytes>,
    pub meta: TransportContext,
}

/// Generic hook action.
///
/// Allows plugins to:
/// - continue
/// - replace data
/// - terminate flows
/// - pause async processing
#[stabby::stabby]
#[repr(u8)]
#[derive(Clone, Copy, Default, Debug)]
pub enum HookAction {
    /// Continue normal runtime execution.
    #[default]
    Continue = 0,
    /// Replace current object/message.
    Replace = 1,
    /// Drop/terminate flow.
    Drop = 2,
}

/// Generic hook result.
#[stabby::stabby]
#[derive(Clone, Copy, Default, Debug)]
pub struct HookOutcome {
    pub action: HookAction,
}
