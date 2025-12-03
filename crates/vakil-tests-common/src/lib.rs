//! Shared helpers for integration/e2e tests used across the workspace.

use async_trait::async_trait;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

pub use pingora_core::upstreams::peer::HttpPeer;
pub use pingora_proxy::Session;
pub use vakil_http::{HttpProxyHooks, ProxySettings, attach_proxy_service};
pub use vakil_plugin_sys::HttpContext;

/// Reserve an available TCP port on localhost.
pub fn reserve_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("reserve port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Block until a TCP service appears on `port` or panic after a short timeout.
pub fn wait_for_port(port: u16) {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return;
        }

        if Instant::now() >= deadline {
            panic!("timed out waiting for port {}", port);
        }

        thread::sleep(Duration::from_millis(50));
    }
}

/// Spawn a minimal backend that asserts the request contains the expected
/// header and path.
pub fn spawn_backend(port: u16, seen_header: Arc<AtomicUsize>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind backend");
        let (mut stream, _) = listener.accept().expect("accept backend request");

        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];

        loop {
            let bytes_read = stream.read(&mut buffer).expect("read backend request");
            if bytes_read == 0 {
                break;
            }

            request.extend_from_slice(&buffer[..bytes_read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.contains("GET /live HTTP/1.1"));
        assert!(request_text.contains("X-Vakil-Proxy: active"));

        seen_header.fetch_add(1, Ordering::SeqCst);

        let response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello proxy";
        stream.write_all(response).expect("write response");
    })
}

/// Spawn a minimal backend that only verifies path and returns a fixed body.
pub fn spawn_backend_plain(port: u16, seen_header: Arc<AtomicUsize>) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind backend");
        let (mut stream, _) = listener.accept().expect("accept backend request");

        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];

        loop {
            let bytes_read = stream.read(&mut buffer).expect("read backend request");
            if bytes_read == 0 {
                break;
            }

            request.extend_from_slice(&buffer[..bytes_read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
        }

        let request_text = String::from_utf8_lossy(&request);
        assert!(request_text.contains("GET /live HTTP/1.1"));

        seen_header.fetch_add(1, Ordering::SeqCst);

        let response =
            b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\nhello proxy";
        stream.write_all(response).expect("write response");
    })
}

/// Spawn a Pingora server with the proxy service attached.
pub fn spawn_proxy(
    proxy_port: u16,
    hooks: impl HttpProxyHooks + 'static,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut server = pingora_core::server::Server::new(None).expect("create pingora server");
        server.bootstrap();

        let settings = ProxySettings::new(format!("127.0.0.1:{}", proxy_port));
        attach_proxy_service(&mut server, settings, hooks);
        server.run_forever();
    })
}

/// Simple TCP request used to exercise the proxy in tests.
pub fn proxy_request(port: u16) -> String {
    proxy_request_result(port).expect("proxy request")
}

/// Simple TCP request used to exercise the proxy in tests, returning I/O
/// errors.
pub fn proxy_request_result(port: u16) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect proxy");
    stream.write_all(b"GET /live HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n")?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

/// Lightweight recording hooks used by tests to assert hooks are invoked.
#[derive(Clone)]
pub struct RecordingHooks {
    pub backend: HttpPeer,
    pub request_filter_calls: Arc<AtomicUsize>,
    pub response_filter_calls: Arc<AtomicUsize>,
}

impl RecordingHooks {
    pub fn new(backend: HttpPeer) -> Self {
        Self {
            backend,
            request_filter_calls: Arc::new(AtomicUsize::new(0)),
            response_filter_calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

#[async_trait]
impl HttpProxyHooks for RecordingHooks {
    async fn request_filter(
        &self,
        _session: &mut Session,
        _ctx: &mut HttpContext,
    ) -> pingora_core::Result<bool> {
        self.request_filter_calls.fetch_add(1, Ordering::SeqCst);
        Ok(false)
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut pingora_http::RequestHeader,
        _ctx: &mut HttpContext,
    ) -> pingora_core::Result<()> {
        upstream_request
            .insert_header("X-Vakil-Proxy", "active")
            .unwrap();
        Ok(())
    }

    async fn response_filter(
        &self,
        _session: &mut Session,
        upstream_response: &mut pingora_http::ResponseHeader,
        _ctx: &mut HttpContext,
    ) -> pingora_core::Result<()> {
        self.response_filter_calls.fetch_add(1, Ordering::SeqCst);
        upstream_response
            .insert_header("X-Vakil-Response", "active")
            .unwrap();
        Ok(())
    }
}
