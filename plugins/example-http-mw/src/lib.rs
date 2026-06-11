use stabby::option::Option as AbiOption;
use stabby::result::Result;
use stabby::string::String;
use stabby::vec::Vec;
use std::option::Option as StdOption;
use vakil_plugin_api::{PluginHttpModule, PluginInstanceOpaque, PluginRootModule};
use vakil_plugin_sys::{
    Bytes, HookAction, HookOutcome, HttpContext, HttpResponse, HttpStatus, ID, PluginError,
    PluginInitContext, PluginManifest, SemVer,
};

struct ModuleHandle {}

fn log_event(name: &str, detail: &str) {
    log::info!("[example-http-mw] {}: {}", name, detail);
}

fn module_mut(inst: *mut PluginInstanceOpaque) -> StdOption<&'static mut ModuleHandle> {
    if inst.is_null() {
        return None;
    }

    unsafe { (inst as *mut ModuleHandle).as_mut() }
}

extern "C" fn http_init(
    inst: *mut PluginInstanceOpaque,
    ctx: *const PluginInitContext,
) -> Result<(), PluginError> {
    if let Some(ctx) = unsafe { ctx.as_ref() } {
        if let Some(module) = module_mut(inst) {
            module;
        }

        log_event(
            "init",
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

extern "C" fn http_on_route(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    let outcome = HookOutcome::default();
    let _http_ctx = unsafe { ctx.as_ref() }.unwrap();
    // TODO: implement changing routes
    Result::Ok(outcome)
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
                stream_id: ID(request.stream_id.0),
                version: vakil_plugin_sys::HttpVersion::Http11,
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
        && let Some(request) = ctx.request.as_mut()
    {
        let status = ctx
            .response
            .as_ref()
            .map(|response| response.status.0)
            .unwrap_or(0);
        log_event(
            "response",
            &format!("path={} status={}", request.path.as_str(), status),
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
    {
        let body_len = ctx
            .response
            .as_ref()
            .map(|response| response.body.0.len())
            .unwrap_or(0);
        log_event(
            "response-body",
            &format!("path={} body={}", request.path.as_str(), body_len),
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
