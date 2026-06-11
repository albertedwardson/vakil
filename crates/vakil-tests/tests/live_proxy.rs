#![allow(dead_code)] // TODO!
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use http::status::StatusCode;
use pingora_core::Result as PingoraResult;
use pingora_proxy::Session;
use vakil_http::HttpProxyHooks;
use vakil_plugin_sys::HttpContext;
use vakil_tests_common::{
    HttpPeer as BackendTarget, RecordingHooks, proxy_request, proxy_request_result, reserve_port,
    spawn_backend, spawn_proxy, wait_for_port,
};

#[derive(Clone, Default)]
struct RejectingHooks;

#[async_trait]
impl HttpProxyHooks for RejectingHooks {
    async fn request_filter(
        &self,
        _session: &mut Session,
        _ctx: &mut HttpContext,
    ) -> PingoraResult<bool> {
        Ok(true)
    }
}

#[derive(Clone, Default)]
struct FilteredHooks;

impl HttpProxyHooks for FilteredHooks {}

#[test]
fn live_proxy_flow_invokes_hooks_and_forwards_response() {
    let _guard = live_proxy_test_guard();

    let backend_port = reserve_port();
    let proxy_port = reserve_port();
    let backend_hits = Arc::new(AtomicUsize::new(0));
    let backend = BackendTarget::new(
        ("127.0.0.1".to_string(), backend_port),
        false,
        "127.0.0.1".to_string(),
    );
    let backend_thread = spawn_backend(backend_port, backend_hits.clone());
    let hooks = RecordingHooks::new(backend.clone());
    let _proxy_thread = spawn_proxy(proxy_port, hooks.clone());

    wait_for_port(proxy_port);

    let response = proxy_request(proxy_port);
    assert!(response.contains("hello proxy"));
    assert!(response.contains("X-Vakil-Response: active"));

    assert_eq!(hooks.request_filter_calls.load(Ordering::SeqCst), 1);
    assert_eq!(hooks.response_filter_calls.load(Ordering::SeqCst), 1);
    assert_eq!(backend_hits.load(Ordering::SeqCst), 1);

    let _ = backend_thread.join();
}

#[test]
fn live_proxy_short_circuits_when_request_filter_rejects() {
    let _guard = live_proxy_test_guard();

    let proxy_port = reserve_port();
    let _backend = BackendTarget::new(
        ("127.0.0.1".to_string(), reserve_port()),
        false,
        "127.0.0.1".to_string(),
    );
    let _proxy_thread = spawn_proxy(proxy_port, RejectingHooks);

    wait_for_port(proxy_port);

    match proxy_request_result(proxy_port) {
        Ok(response) => {
            assert!(response.contains(&StatusCode::FORBIDDEN.as_u16().to_string()));
        }
        Err(err) => {
            assert_eq!(err.kind(), std::io::ErrorKind::ConnectionReset);
        }
    }
}

#[test]
fn live_proxy_drops_tcp_connections_when_connection_filter_rejects() {
    let _guard = live_proxy_test_guard();

    let proxy_port = reserve_port();
    let _backend = BackendTarget::new(
        ("127.0.0.1".to_string(), reserve_port()),
        false,
        "127.0.0.1".to_string(),
    );
    let _proxy_thread = spawn_proxy(proxy_port, FilteredHooks);

    wait_for_port(proxy_port);

    match proxy_request_result(proxy_port) {
        Ok(response) => assert!(response.is_empty()),
        Err(_err) => {}
    }
}

fn wait_for_counter(counter: &AtomicUsize, expected: usize, label: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if counter.load(Ordering::SeqCst) >= expected {
            return;
        }

        if Instant::now() >= deadline {
            panic!("timed out waiting for {}", label);
        }

        std::thread::sleep(Duration::from_millis(50));
    }
}

fn build_example_urg_stripper_plugin() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "-p", "example-urg-stripper"])
        .status()
        .expect("cargo build example-urg-stripper");
    assert!(status.success(), "example-urg-stripper build failed");

    locate_dynamic_library("example_urg_stripper")
}

fn socket_address(addr: &SocketAddr) -> vakil_plugin_sys::SocketAddress {
    vakil_plugin_sys::SocketAddress {
        host: addr.ip().to_string().into(),
        port: addr.port(),
    }
}

fn send_oob_byte(stream: &TcpStream, byte: u8) {
    let fd = stream.as_raw_fd();
    let buffer = [byte];
    let sent = unsafe { libc::send(fd, buffer.as_ptr().cast(), 1, libc::MSG_OOB) };
    assert_eq!(sent, 1, "send MSG_OOB byte");
}

fn sock_at_mark(stream: &TcpStream) -> bool {
    let fd = stream.as_raw_fd();
    const SIOCATMARK: libc::c_ulong = 0x8905;
    let mut at_mark: libc::c_int = 0;
    let result = unsafe { libc::ioctl(fd, SIOCATMARK, &mut at_mark) };
    assert!(
        result >= 0,
        "sockatmark failed: {}",
        std::io::Error::last_os_error()
    );
    at_mark != 0
}

fn spawn_urgent_backend(
    port: u16,
    urgent_mark_seen: Arc<AtomicUsize>,
    ready_tx: std::sync::mpsc::Sender<()>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind backend");
        ready_tx.send(()).expect("signal backend ready");
        let (mut stream, _) = listener.accept().expect("accept backend");

        let mut first = [0u8; 1];
        stream.read_exact(&mut first).expect("read first byte");
        assert_eq!(first[0], b'A');

        if sock_at_mark(&stream) {
            urgent_mark_seen.store(1, Ordering::SeqCst);
        }

        let mut second = [0u8; 1];
        stream.read_exact(&mut second).expect("read second byte");
        assert_eq!(second[0], b'B');

        assert!(
            !sock_at_mark(&stream),
            "urgent mark should not survive stripping"
        );

        stream.write_all(b"backend-ok").expect("write response");
    })
}

fn live_proxy_test_guard() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("lock live_proxy tests")
}

fn locate_dynamic_library(stem: &str) -> PathBuf {
    let target_dir = workspace_target_debug_dir();
    let entries = fs::read_dir(&target_dir)
        .unwrap_or_else(|err| panic!("read target dir {}: {}", target_dir.display(), err));

    for entry in entries {
        let entry = entry.expect("target entry");
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };

        if !name.contains(stem) {
            continue;
        }

        if is_dynamic_library(&path) {
            return path;
        }
    }

    panic!(
        "could not locate built dynamic library for {} in {}",
        stem,
        target_dir.display()
    );
}

fn workspace_target_debug_dir() -> PathBuf {
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        return Path::new(&target_dir).join("debug");
    }

    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target")
        .join("debug")
}

fn is_dynamic_library(path: &Path) -> bool {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("so") if cfg!(target_os = "linux") => true,
        Some("dylib") if cfg!(target_os = "macos") => true,
        Some("dll") if cfg!(target_os = "windows") => true,
        _ => false,
    }
}
