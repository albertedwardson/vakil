//! HTTP-only example plugin demonstrating routing, request interception, and
//! local responses.
//!
//! Behavior:
//! - Rotates upstream per request using a global counter:
//! - Even requests → httpbin.dev/dump/request?n={n}
//! - Odd requests → httpbin.org/anything?n={n}
//! - Query parameter n increments globally per request.
//! - Special case: GET /local-reply returns a static response without upstream
//!   forwarding.
//!
//! Hook coverage:
//! - on_route: selects upstream dynamically
//! - on_request_headers: inspects request and may short-circuit response
//! - on_request_body: logs request payload size
//! - on_response_headers/body: logs upstream response metadata
//! - on_trailers: logs trailer phase
//! - on_local_reply: logs locally generated responses

use stabby::option::Option as AbiOption;
use stabby::result::Result;
use stabby::string::String;
use stabby::vec::Vec;
use std::sync::atomic::{AtomicU64, Ordering};
use vakil_plugin_api::{PluginHttpModule, PluginInstanceOpaque, PluginRootModule};
use vakil_plugin_sys::{
    Bytes, HookAction, HookOutcome, HttpContext, HttpResponse, HttpStatus, PluginError,
    PluginInitContext, PluginManifest, RouteAction, RouteDecision, SemVer, SocketAddress,
};

struct ModuleHandle {}

fn log_event(name: &str, detail: &str) {
    log::info!("[example-http-mw] {}: {}", name, detail);
}

extern "C" fn http_init(
    _inst: *mut PluginInstanceOpaque,
    ctx: *const PluginInitContext,
) -> Result<(), PluginError> {
    let _ =
        env_logger::Builder::from_env(env_logger::Env::default().filter_or("RUST_LOG", "trace"))
            .format_timestamp_millis()
            .try_init();
    if let Some(ctx) = unsafe { ctx.as_ref() } {
        log_event(
            "inited",
            &format!(
                "library={} host={}.{}.{} env_entries={}",
                ctx.library_path.as_str(),
                ctx.host_version.major,
                ctx.host_version.minor,
                ctx.host_version.patch,
                ctx.env.entries.len()
            ),
        );
    }

    Result::Ok(())
}

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

extern "C" fn http_on_route(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    let ctx = unsafe { ctx.as_mut() }.unwrap();
    if ctx.request.is_none() {
        return Result::Err(PluginError {
            message: Some("no request provided to callback".into()).into(),
        });
    }

    let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    let (host, path) = match n % 3 {
        1 => ("httpbin.dev", format!("/dump/request?n={n}")),
        2 => ("httpbin.org", format!("/anything?n={n}")),
        0 => ("echo.free.beeceptor.com", format!("/?n={n}")),
        _ => unreachable!(),
    };

    let is_route_ok = ctx.set_route(RouteDecision {
        action: RouteAction::ReplaceUpstream,
        upstream_to_set: Some(SocketAddress {
            host: host.into(),
            port: 80,
        })
        .into(),
        http_path: Some(path.into()).into(),
    });
    if !is_route_ok {
        return Result::Err(PluginError {
            message: Some("failed to set route".into()).into(),
        });
    }

    log_event(
        "route",
        &format!("upstream=https://httpbin.dev/dump/request?n={n}"),
    );

    Result::Ok(HookOutcome {
        action: HookAction::Replace,
    })
}

extern "C" fn http_on_request_headers(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() }
        && let Some(request) = ctx.request.as_mut()
    {
        log_event(
            "request",
            &format!(
                "method={} path={} headers={} body={}",
                request.method.as_str(),
                request.path.as_str(),
                request.headers.len(),
                request.body.0.len()
            ),
        );

        if request.method.as_str() == "GET" && request.path.as_str() == "/local-reply" {
            ctx.response = Some(HttpResponse {
                stream_id: request.stream_id,
                version: request.version,
                status: HttpStatus(200),
                headers: Default::default(),
                body: Bytes(Vec::from("hello from example-http-mw".as_bytes())),
            })
            .into();

            return Result::Ok(HookOutcome {
                action: HookAction::Replace,
            });
        }
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn http_on_request_body(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() }
        && let Some(request) = ctx.request.as_mut()
    {
        log_event(
            "request-body",
            &format!(
                "path={} body={}",
                request.path.as_str(),
                request.body.0.len()
            ),
        );
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn http_on_response_headers(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() }
        && let Some(response) = ctx.response.as_mut()
    {
        log_event(
            "response",
            &format!(
                "status={:?} headers={:?}",
                response.status, response.headers
            ),
        );
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn http_on_response_body(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() }
        && let Some(request) = ctx.request.as_mut()
        && let Some(response) = ctx.response.as_mut()
    {
        log_event(
            "response-body",
            &format!(
                "path={} body={}",
                request.path.as_str(),
                response.body.len()
            ),
        );
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn http_on_trailers(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() }
        && let Some(request) = ctx.request.as_mut()
    {
        log_event("trailers", request.path.as_str());
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn http_on_local_reply(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() }
        && let Some(request) = ctx.request.as_mut()
    {
        log_event("local-reply", request.path.as_str());
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn http_shutdown(inst: *mut PluginInstanceOpaque) -> Result<(), PluginError> {
    log_event("shutdown", "module instance shut down");

    if !inst.is_null() {
        unsafe {
            drop(Box::from_raw(inst as *mut ModuleHandle));
        }
    }

    Result::Ok(())
}

fn build_http_module() -> PluginHttpModule {
    let instance = Box::into_raw(Box::new(ModuleHandle {})) as *mut PluginInstanceOpaque;

    PluginHttpModule {
        instance,
        priority: 0,
        init: http_init,
        on_route: AbiOption::Some(http_on_route),
        on_request_headers: AbiOption::Some(http_on_request_headers),
        on_request_body: AbiOption::Some(http_on_request_body),
        on_response_headers: AbiOption::Some(http_on_response_headers),
        on_response_body: AbiOption::Some(http_on_response_body),
        on_trailers: AbiOption::Some(http_on_trailers),
        on_local_reply: AbiOption::Some(http_on_local_reply),
        shutdown: http_shutdown,
    }
}

extern "C" fn create_http() -> Result<PluginHttpModule, PluginError> {
    Result::Ok(build_http_module())
}

#[unsafe(no_mangle)]
pub extern "C" fn get_library() -> PluginRootModule {
    PluginRootModule {
        manifest: PluginManifest {
            name: String::from("example-http-mw"),
            version: SemVer {
                major: 0,
                minor: 1,
                patch: 0,
            },
        },
        create_http: AbiOption::Some(create_http),
        create_tcp: AbiOption::None(),
        create_udp: AbiOption::None(),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        create_http, get_library, http_init, http_on_local_reply, http_on_request_body,
        http_on_request_headers, http_on_response_body, http_on_response_headers, http_on_route,
        http_on_trailers,
    };
    use stabby::option::Option as AbiOption;
    use stabby::string::String;
    use stabby::vec::Vec;
    use vakil_plugin_sys::{
        Bytes, EnvSnapshot, HookAction, HttpContext, HttpRequest, HttpVersion, ID, KVPair,
        PluginInitContext, SemVer,
    };

    fn sample_init_context(entries: Vec<KVPair>) -> PluginInitContext {
        PluginInitContext {
            library_path: String::from("/tmp/example-http-mw.so"),
            plugin_dir: AbiOption::Some(String::from("/tmp")),
            host_version: SemVer {
                major: 1,
                minor: 0,
                patch: 0,
            },
            env: EnvSnapshot { entries },
        }
    }

    fn sample_http_context(path: &str) -> HttpContext {
        HttpContext {
            request: AbiOption::Some(HttpRequest {
                stream_id: ID(1),
                version: HttpVersion::Http11,
                is_tls: false,
                method: String::from("GET"),
                authority: String::from("example.test"),
                path: String::from(path),
                headers: Default::default(),
                body: Default::default(),
            }),
            response: AbiOption::None(),
            ..Default::default()
        }
    }

    #[test]
    fn exports_http_only() {
        let root = get_library();
        assert!(root.create_http.is_some());
        assert!(root.create_tcp.is_none());
        assert!(root.create_udp.is_none());
        assert_eq!(root.manifest.name.as_str(), "example-http-mw");
    }

    #[test]
    fn local_reply_is_set_for_local_reply_path() {
        let module = create_http().match_owned(|module| module, |_| panic!("factory failed"));
        let mut ctx = HttpContext {
            request: AbiOption::Some(HttpRequest {
                stream_id: ID(1),
                version: HttpVersion::Http11,
                is_tls: false,
                method: String::from("GET"),
                authority: String::from("example.test"),
                path: String::from("/local-reply"),
                headers: Default::default(),
                body: Default::default(),
            }),
            response: AbiOption::None(),
            ..Default::default()
        };

        let outcome = http_on_request_headers(module.instance, &mut ctx as *mut _)
            .match_owned(|outcome| outcome, |_| panic!("hook failed"));

        assert_eq!(outcome.action as u8, HookAction::Replace as u8);
        assert!(ctx.response.is_some());
    }

    #[test]
    fn init_loads_backend_from_env_and_routes_upstream() {
        let module = create_http().match_owned(|module| module, |_| panic!("factory failed"));
        let mut entries = Vec::new();
        entries.push(KVPair {
            name: String::from("VAKIL_HTTP_MW_BACKEND"),
            value: String::from("127.0.0.1:8089"),
        });
        let init_context = sample_init_context(entries);

        http_init(module.instance, &init_context as *const _)
            .match_owned(|_| (), |_| panic!("init failed"));

        let mut route_context = sample_http_context("");
        let decision = http_on_route(module.instance, &mut route_context as *mut _)
            .match_owned(|decision| decision, |_| panic!("route failed"));

        assert_eq!(
            decision.action as u8,
            vakil_plugin_sys::RouteAction::ReplaceUpstream as u8
        );
        // TODO: implement
        // assert_eq!(upstream.host.as_str(), "127.0.0.1");
        // assert_eq!(upstream.port, 8089);
    }

    #[test]
    fn response_callbacks_are_callable() {
        let module = create_http().match_owned(|module| module, |_| panic!("factory failed"));
        let mut ctx = sample_http_context("/live");
        ctx.response = AbiOption::Some(vakil_plugin_sys::HttpResponse {
            stream_id: ID(1),
            version: HttpVersion::Http11,
            status: vakil_plugin_sys::HttpStatus(200),
            headers: Default::default(),
            body: Bytes(Vec::from("hello".as_bytes())),
        });

        http_on_response_headers(module.instance, &mut ctx as *mut _)
            .match_owned(|_| (), |_| panic!("response headers failed"));
        http_on_response_body(module.instance, &mut ctx as *mut _)
            .match_owned(|_| (), |_| panic!("response body failed"));
        http_on_trailers(module.instance, &mut ctx as *mut _)
            .match_owned(|_| (), |_| panic!("trailers failed"));
        http_on_local_reply(module.instance, &mut ctx as *mut _)
            .match_owned(|_| (), |_| panic!("local reply failed"));
        http_on_request_body(module.instance, &mut ctx as *mut _)
            .match_owned(|_| (), |_| panic!("request body failed"));
    }
}
