use pingora_http::RequestHeader;
use stabby::option::Option as AbiOption;
use stabby::string::String as AbiString;
use stabby::vec::Vec as AbiVec;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::thread;
use vakil_http::VakilHttpProxy;
use vakil_plugin_api::{HttpHookFn, PluginInstanceOpaque};
use vakil_plugin_host::LoadedPlugin;
use vakil_plugin_sys::{HookAction, HookOutcome, KVPair, RouteAction, RouteDecision};
use vakil_tests_common::{
    ProxySettings, attach_proxy_service, reserve_port, spawn_backend_plain, wait_for_port,
};

// Additional imports required for the HttpProxyHooks implementation below.
use bytes::Bytes;
use pingora_core::Result;
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use vakil_http::HttpProxyHooks;
use vakil_plugin_sys::HttpContext;

struct LoadedHttpPluginHooks {
    instance: *mut PluginInstanceOpaque,
    on_route: AbiOption<HttpHookFn>,
    on_request_headers: AbiOption<HttpHookFn>,
    on_response_headers: AbiOption<HttpHookFn>,
    on_local_reply: AbiOption<HttpHookFn>,
}

unsafe impl Send for LoadedHttpPluginHooks {}
unsafe impl Sync for LoadedHttpPluginHooks {}

// Implement the HttpProxyHooks trait for the loaded plugin hooks.
// For now we provide no‑op implementations that simply satisfy the trait
// requirements. Real hook logic can be added later by invoking the stored
// C‑ABI callbacks.
#[async_trait::async_trait]
impl HttpProxyHooks for LoadedHttpPluginHooks {
    async fn request_filter(&self, _session: &mut Session, _ctx: &mut HttpContext) -> Result<bool> {
        // In a full implementation we would call `self.on_route` etc.
        Ok(false)
    }

    async fn request_body_filter(
        &self,
        _session: &mut Session,
        _body: &mut Option<Bytes>,
        _end_of_stream: bool,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        _upstream_request: &mut RequestHeader,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    async fn response_filter(
        &self,
        _session: &mut Session,
        _upstream_response: &mut ResponseHeader,
        _ctx: &mut HttpContext,
    ) -> Result<()> {
        Ok(())
    }

    fn response_body_filter(
        &self,
        _session: &mut Session,
        _body: &mut Option<Bytes>,
        _end_of_stream: bool,
        _ctx: &mut HttpContext,
    ) -> Result<Option<std::time::Duration>> {
        Ok(None)
    }

    async fn logging(
        &self,
        _session: &mut Session,
        _error: Option<&pingora_core::Error>,
        _ctx: &mut HttpContext,
    ) {
        // No logging needed for the test.
    }
}
// Alias that makes the proxy type easier to refer to
type LoadedHttpPluginProxy = VakilHttpProxy<LoadedHttpPluginHooks>;

fn continue_hook() -> HookOutcome {
    HookOutcome {
        action: HookAction::Continue,
    }
}

fn keep_route_decision() -> RouteDecision {
    RouteDecision {
        upstream_to_set: AbiOption::None(),
        action: RouteAction::Keep,
    }
}

fn build_headers(req: &RequestHeader) -> AbiVec<KVPair> {
    let mut headers = AbiVec::new();

    for (name, value) in req.headers.iter() {
        headers.push(KVPair {
            name: AbiString::from(name.as_str()),
            value: AbiString::from(value.to_str().unwrap_or("")),
        });
    }

    headers
}

fn build_example_noop_plugin() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "-p", "example-noop"])
        .status()
        .expect("cargo build example-noop");
    assert!(status.success(), "example-noop build failed");

    locate_dynamic_library("example_noop")
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

fn http_request(port: u16, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port))?;
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n",
        path
    );
    stream.write_all(request.as_bytes())?;

    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    Ok(response)
}

fn load_http_plugin() -> LoadedHttpPluginHooks {
    let libpath = build_example_noop_plugin();
    let mut plugin = LoadedPlugin::load(&libpath).expect("load example-noop plugin");
    assert_eq!(plugin.name(), "example-noop");
    assert!(plugin.has_http());
    assert!(plugin.has_tcp());
    assert!(plugin.has_udp());

    let http_module = plugin.modules.http.take().expect("http module");
    let vakil_plugin_api::PluginHttpModule {
        instance,
        on_route,
        on_request_headers,
        on_response_headers,
        on_local_reply,
        ..
    } = http_module;
    std::mem::forget(plugin);

    LoadedHttpPluginHooks {
        instance,
        on_route,
        on_request_headers,
        on_response_headers,
        on_local_reply,
    }
}

#[test]
fn live_proxy_executes_loaded_plugin_and_routes_real_http_traffic() {
    let backend_port = reserve_port();
    let proxy_port = reserve_port();
    let backend_hits = Arc::new(AtomicUsize::new(0));

    let backend_thread = spawn_backend_plain(backend_port, backend_hits.clone());

    let hooks = load_http_plugin();
    let _proxy_thread = thread::spawn(move || {
        let mut server = pingora_core::server::Server::new(None).expect("create pingora server");
        server.bootstrap();

        let settings = ProxySettings::new(format!("127.0.0.1:{}", proxy_port));
        attach_proxy_service(&mut server, settings, hooks);
        server.run_forever();
    });

    wait_for_port(proxy_port);

    let routed_response = http_request(proxy_port, "/live").expect("live request");
    assert!(routed_response.contains("hello proxy"));

    let local_reply = http_request(proxy_port, "/local-reply").expect("local reply request");
    assert!(local_reply.contains("hello from example-noop"));

    assert_eq!(backend_hits.load(Ordering::SeqCst), 1);

    let _ = backend_thread.join();
}
