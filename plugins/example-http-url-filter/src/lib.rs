use ada_url::Idna;
use ada_url::Url;
use stabby::option::Option as AbiOption;
use stabby::result::Result;
use stabby::string::String;
use stabby::vec::Vec;
use vakil_plugin_api::{PluginHttpModule, PluginInstanceOpaque, PluginRootModule};
use vakil_plugin_sys::{
    Bytes, HookAction, HookOutcome, HttpContext, HttpResponse, HttpStatus, PluginError,
    PluginInitContext, PluginManifest, SemVer,
};

macro_rules! plugin_log {
    ($level:ident, $fmt:literal $(, $args:expr)* $(,)?) => {
        log::$level!(concat!("[", env!("CARGO_PKG_NAME"), "] ", $fmt) $(, $args)*);
    };
}

struct ModuleHandle;

fn continue_hook() -> HookOutcome {
    HookOutcome {
        action: HookAction::Continue,
    }
}

fn reject_with_bad_request(ctx: &HttpContext) -> HttpResponse {
    let req = ctx.request.clone().unwrap();
    HttpResponse {
        stream_id: req.stream_id,
        version: req.version,
        status: HttpStatus(400),
        headers: Default::default(),
        body: Bytes(Vec::from("bad request: invalid URL".as_bytes())),
    }
}

fn request_target_url(ctx: &HttpContext) -> std::string::String {
    let req = ctx.request.clone().unwrap();
    let authority = req.authority.as_str();

    if authority.is_empty() {
        format!("http://localhost{}", req.path.as_str())
    } else {
        format!("http://{authority}{}", req.path.as_str())
    }
}

fn authority_is_valid(authority: &str) -> bool {
    if authority.is_empty() {
        return false;
    }

    if authority.starts_with('[') {
        return true;
    }

    let host = authority
        .rsplit_once(':')
        .map_or(authority, |(host, port)| {
            if port.chars().all(|ch| ch.is_ascii_digit()) {
                host
            } else {
                authority
            }
        });

    !host.is_empty()
        && host
            .chars()
            .all(|ch| !ch.is_whitespace() && !ch.is_control())
        && !Idna::ascii(host).is_empty()
}

fn request_target_is_valid(ctx: &HttpContext) -> bool {
    let req = ctx.request.clone().unwrap();

    if !authority_is_valid(req.authority.as_str()) {
        return false;
    }

    let request_target = request_target_url(ctx);
    Url::can_parse(request_target.as_str(), None)
}

extern "C" fn http_init(
    _inst: *mut PluginInstanceOpaque,
    _ctx: *const PluginInitContext,
) -> Result<(), PluginError> {
    Result::Ok(())
}

extern "C" fn http_on_request_headers(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else {
        return Result::Ok(continue_hook());
    };
    let req = ctx.request.clone().unwrap();

    if !request_target_is_valid(ctx) {
        plugin_log!(
            warn,
            "rejecting invalid request target {}{}",
            req.authority.as_str(),
            req.path.as_str()
        );
        ctx.response = AbiOption::Some(reject_with_bad_request(ctx));
        return Result::Ok(HookOutcome {
            action: HookAction::Replace,
        });
    }

    plugin_log!(
        debug,
        "accepted request target {}{}",
        req.authority.as_str(),
        req.path.as_str()
    );

    Result::Ok(continue_hook())
}

extern "C" fn http_shutdown(_inst: *mut PluginInstanceOpaque) -> Result<(), PluginError> {
    Result::Ok(())
}

fn build_http_module() -> PluginHttpModule {
    let instance = Box::into_raw(Box::new(ModuleHandle)) as *mut PluginInstanceOpaque;

    PluginHttpModule {
        instance,
        priority: 0,
        init: http_init,
        shutdown: http_shutdown,
        on_route: AbiOption::None(),
        on_request_headers: AbiOption::Some(http_on_request_headers),
        on_request_body: AbiOption::None(),
        on_response_headers: AbiOption::None(),
        on_response_body: AbiOption::None(),
        on_trailers: AbiOption::None(),
        on_local_reply: AbiOption::None(),
    }
}

extern "C" fn create_http() -> Result<PluginHttpModule, PluginError> {
    Result::Ok(build_http_module())
}

#[unsafe(no_mangle)]
pub extern "C" fn get_library() -> PluginRootModule {
    PluginRootModule {
        manifest: PluginManifest {
            name: String::from("example-http-url-filter"),
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
    use super::{build_http_module, create_http, get_library, http_on_request_headers};
    use stabby::option::Option as AbiOption;
    use stabby::string::String;
    use vakil_plugin_sys::{HookAction, HttpContext, HttpRequest, HttpVersion, ID};

    fn sample_context(authority: &str, path: &str) -> HttpContext {
        HttpContext {
            request: Some(HttpRequest {
                stream_id: ID(69),
                version: HttpVersion::Http11,
                method: String::from("GET"),
                authority: String::from(authority),
                path: String::from(path),
                headers: Default::default(),
                body: Default::default(),
            })
            .into(),
            response: AbiOption::None(),
            ..Default::default()
        }
    }

    #[test]
    fn exports_http_only_plugin() {
        let library = get_library();

        assert_eq!(library.manifest.name.as_str(), "example-http-url-filter");
        assert!(library.create_http.is_some());
        assert!(library.create_tcp.is_none());
        assert!(library.create_udp.is_none());
        assert!(create_http().is_ok());
    }

    #[test]
    fn valid_request_target_continues() {
        let module = build_http_module();
        let mut ctx = sample_context("example.test", "/search");

        let outcome = http_on_request_headers(module.instance, &mut ctx as *mut _)
            .match_owned(|outcome| outcome, |_| panic!("hook failed"));

        assert_eq!(outcome.action as u8, HookAction::Continue as u8);
        assert!(ctx.response.is_none());
    }

    #[test]
    fn invalid_request_target_rejects_with_400() {
        let module = build_http_module();
        let mut ctx = sample_context("exa[ ]mple.test", "/bad");

        let outcome = http_on_request_headers(module.instance, &mut ctx as *mut _)
            .match_owned(|outcome| outcome, |_| panic!("hook failed"));

        assert_eq!(outcome.action as u8, HookAction::Replace as u8);
        let response = ctx
            .response
            .match_owned(|response| response, || panic!("missing response"));
        assert_eq!(response.status.0, 400);
        assert_eq!(response.body.0.as_slice(), b"bad request: invalid URL");
    }

    #[test]
    fn invalid_host_is_rejected_before_url_validation() {
        assert!(!super::authority_is_valid("exa mple.test"));
        assert!(super::authority_is_valid("example.test"));
    }
}
