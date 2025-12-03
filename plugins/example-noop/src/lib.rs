use stabby::option::Option;
use stabby::result::Result;
use stabby::string::String;
use stabby::vec::Vec;
use vakil_plugin_api::{
    PluginHttpModule, PluginInstanceOpaque, PluginRootModule, PluginTcpModule, PluginUdpModule,
};
use vakil_plugin_sys::{
    Bytes, HookAction, HookOutcome, HttpContext, HttpResponse, HttpStatus, ID, PluginError,
    PluginInitContext, PluginManifest, SemVer, TCPContext, UDPContext,
};

#[inline]
fn log_event(name: &str, detail: &str) {
    log::info!("[example-noop] {name}: {detail}");
}

extern "C" fn example_init(
    _inst: *mut PluginInstanceOpaque,
    ctx: *const PluginInitContext,
) -> Result<(), PluginError> {
    if let Some(ctx) = unsafe { ctx.as_ref() } {
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
    } else {
        log::warn!("[example-noop] init: received null PluginInitContext");
    }

    Result::Ok(())
}

// TODO: implement this
// extern "C" fn example_on_route(
//     _inst: *mut PluginInstanceOpaque,
//     ctx: *mut HttpRouteContext,
// ) -> Result<RouteDecision, PluginError> {
//     if let Some(ctx) = unsafe { ctx.as_mut() } {
//         let hint = if ctx.route_hint.is_some() {
//             "some"
//         } else {
//             "none"
//         };

//         log_event(
//             "route",
//             &format!(
//                 "listener={} peer={} proto={} route_hint={}",
//                 ctx.listener.as_str(),
//                 ctx.peer.as_str(),
//                 match &ctx.protocol {
//                     Protocol::Http => 0,
//                     Protocol::Tcp => 1,
//                     Protocol::Udp => 2,
//                 },
//                 hint
//             ),
//         );
//     } else {
//         log::warn!("[example-noop] route: received null RouteContext");
//     }

//     Result::Ok(keep_route())
// }
extern "C" fn example_on_http_event(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut HttpContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() } {
        let response = if ctx.response.is_some() {
            "present"
        } else {
            "none"
        };
        let req = ctx.request.clone().unwrap();
        log_event(
            "http",
            &format!(
                "request={} {} headers={} response={}",
                req.method.as_str(),
                req.path.as_str(),
                req.headers.len(),
                response
            ),
        );

        if req.method.as_str() == "GET"
            && req.path.as_str() == "/local-reply"
            && ctx.response.is_none()
        {
            ctx.response = Option::Some(HttpResponse {
                stream_id: ID(req.stream_id.0),
                version: req.version,
                status: HttpStatus(200),
                headers: Default::default(),
                body: Bytes(Vec::from("hello from example-noop".as_bytes())),
            });

            return Result::Ok(HookOutcome {
                action: HookAction::Replace,
            });
        }
    } else {
        log::warn!("[example-noop] http: received null HttpContext");
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn example_on_tcp_event(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut TCPContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() } {
        log_event(
            "tcp",
            &format!("payload_bytes={}", ctx.chunk.clone().unwrap().0.len()),
        );
    } else {
        log::warn!("[example-noop] tcp: received null TcpContext");
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn example_on_udp_event(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut UDPContext,
) -> Result<HookOutcome, PluginError> {
    if let Some(ctx) = unsafe { ctx.as_mut() } {
        log_event(
            "udp",
            &format!(
                "peer={} payload_bytes={}",
                ctx.meta.connection.peer_addr.match_mut(
                    |v| { std::string::String::from((*v).clone()) },
                    || { "".into() }
                ),
                ctx.datagram.match_mut(|v| { v.0.len() }, || { 0 }),
            ),
        );
    } else {
        log::warn!("[example-noop] udp: received null UdpContext");
    }

    Result::Ok(HookOutcome::default())
}

extern "C" fn example_shutdown(_inst: *mut PluginInstanceOpaque) -> Result<(), PluginError> {
    log_event("shutdown", "module instance shut down");
    Result::Ok(())
}

fn build_http_module() -> PluginHttpModule {
    PluginHttpModule {
        instance: std::ptr::null_mut(),
        priority: 0,
        init: example_init,
        on_route: Default::default(),
        on_request_headers: Option::Some(example_on_http_event),
        on_request_body: Option::Some(example_on_http_event),
        on_response_headers: Option::Some(example_on_http_event),
        on_response_body: Option::Some(example_on_http_event),
        on_trailers: Option::Some(example_on_http_event),
        on_local_reply: Option::Some(example_on_http_event),
        shutdown: example_shutdown,
    }
}

fn build_tcp_module() -> PluginTcpModule {
    PluginTcpModule {
        instance: std::ptr::null_mut(),
        priority: 1,
        init: example_init,
        on_route: Default::default(),
        on_connect: Option::Some(example_on_tcp_event),
        on_data: Option::Some(example_on_tcp_event),
        on_half_close: Option::Some(example_on_tcp_event),
        on_close: Option::Some(example_on_tcp_event),
        shutdown: example_shutdown,
    }
}

fn build_udp_module() -> PluginUdpModule {
    PluginUdpModule {
        instance: std::ptr::null_mut(),
        priority: 2,
        init: example_init,
        on_route: Default::default(),
        on_datagram: Option::Some(example_on_udp_event),
        on_session_start: Option::Some(example_on_udp_event),
        on_session_end: Option::Some(example_on_udp_event),
        shutdown: example_shutdown,
    }
}

extern "C" fn create_http() -> Result<PluginHttpModule, PluginError> {
    Result::Ok(build_http_module())
}

extern "C" fn create_tcp() -> Result<PluginTcpModule, PluginError> {
    Result::Ok(build_tcp_module())
}

extern "C" fn create_udp() -> Result<PluginUdpModule, PluginError> {
    Result::Ok(build_udp_module())
}

#[unsafe(no_mangle)]
pub extern "C" fn get_library() -> PluginRootModule {
    PluginRootModule {
        manifest: PluginManifest {
            name: String::from("example-noop"),
            version: SemVer {
                major: 0,
                minor: 1,
                patch: 0,
            },
        },
        create_http: Option::Some(create_http),
        create_tcp: Option::Some(create_tcp),
        create_udp: Option::Some(create_udp),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        build_http_module, build_tcp_module, build_udp_module, create_http, create_tcp, create_udp,
        example_on_http_event, example_on_tcp_event, example_on_udp_event, get_library,
    };
    use stabby::option::Option;
    use stabby::string::String;
    use stabby::vec::Vec;
    use vakil_plugin_sys::{
        Bytes, ConnectionInfo, EnvSnapshot, HookAction, HookOutcome, HttpContext, HttpRequest, ID,
        PluginInitContext, RouteAction, RouteDecision, SemVer, SocketAddress, TCPContext,
        TransportContext, TransportProtocol, UDPContext,
    };

    fn sample_init_context() -> PluginInitContext {
        PluginInitContext {
            library_path: String::from("/tmp/example-noop.so"),
            plugin_dir: Option::Some(String::from("/tmp")),
            host_version: SemVer {
                major: 1,
                minor: 0,
                patch: 0,
            },
            env: EnvSnapshot {
                entries: Vec::new(),
            },
        }
    }

    fn sample_http_context() -> HttpContext {
        HttpContext {
            request: Some(HttpRequest {
                stream_id: ID(1),
                version: vakil_plugin_sys::HttpVersion::Http11,
                method: String::from("GET"),
                authority: String::from("example.com"),
                path: String::from("/health"),
                headers: Default::default(),
                body: Bytes(Vec::new()),
            })
            .into(),
            response: Option::None(),
            ..Default::default()
        }
    }

    fn sample_tcp_context() -> TCPContext {
        TCPContext {
            chunk: Some(Bytes(Vec::from(&[1, 2, 3, 4][..]))).into(),
            meta: TransportContext {
                connection: ConnectionInfo {
                    id: ID(1),
                    local_addr: SocketAddress {
                        host: String::from("127.0.0.1"),
                        port: 8080,
                    },
                    peer_addr: Some(SocketAddress {
                        host: String::from("127.0.0.1"),
                        port: 54321,
                    })
                    .into(),
                    protocol: TransportProtocol::Tcp,
                },
                direction: None.into(),
                route: Default::default(),
            },
        }
    }

    fn sample_udp_context() -> UDPContext {
        UDPContext {
            datagram: Some(Bytes(Vec::from(&[1, 2, 3, 4][..]))).into(),
            meta: TransportContext {
                connection: ConnectionInfo {
                    id: ID(1),
                    local_addr: SocketAddress {
                        host: String::from("127.0.0.1"),
                        port: 8080,
                    },
                    peer_addr: Some(SocketAddress {
                        host: String::from("127.0.0.1"),
                        port: 54321,
                    })
                    .into(),
                    protocol: TransportProtocol::Udp,
                },
                direction: None.into(),
                route: Default::default(),
            },
        }
    }

    #[test]
    fn get_library_exports_all_protocol_modules() {
        let library = get_library();

        assert_eq!(library.manifest.name.as_str(), "example-noop");
        assert!(create_http().is_ok());
        assert!(create_tcp().is_ok());
        assert!(create_udp().is_ok());
    }

    #[test]
    fn http_callbacks_are_callable_with_real_and_null_contexts() {
        let module = build_http_module();
        let init_context = sample_init_context();
        let mut http_ctx = sample_http_context();

        assert!((module.init)(module.instance, &init_context as *const _).is_ok());
        assert!((module.init)(module.instance, core::ptr::null()).is_ok());

        // TODO
        // let route = example_on_route(module.instance, &mut http_ctx as *mut _)
        //     .match_owned(|route| route, |_| panic!("route error"));
        // assert_eq!(route.action as u8, RouteAction::Keep as u8);
        // assert!(route.upstream_to_set.is_none());

        // let route_null = example_on_route(module.instance, core::ptr::null_mut())
        //     .match_owned(|route| route, |_| panic!("route error"));
        // assert_eq!(route_null.action as u8, RouteAction::Keep as u8);

        let outcome = example_on_http_event(module.instance, &mut http_ctx as *mut _)
            .match_owned(|outcome| outcome, |_| panic!("hook error"));
        assert_eq!(outcome.action as u8, HookAction::Continue as u8);

        let null_outcome = example_on_http_event(module.instance, core::ptr::null_mut())
            .match_owned(|outcome| outcome, |_| panic!("hook error"));
        assert_eq!(null_outcome.action as u8, HookAction::Continue as u8);

        (module.shutdown)(module.instance);
    }

    #[test]
    fn http_request_can_short_circuit_with_local_reply() {
        let module = build_http_module();
        let mut http_ctx = HttpContext {
            request: Some(HttpRequest {
                stream_id: ID(1),
                version: vakil_plugin_sys::HttpVersion::Http11,
                method: String::from("GET"),
                authority: String::from("example.com"),
                path: String::from("/local-reply"),
                headers: Default::default(),
                body: Bytes(Vec::new()),
            })
            .into(),
            response: Option::None(),
            ..Default::default()
        };

        let outcome = example_on_http_event(module.instance, &mut http_ctx as *mut _)
            .match_owned(|outcome| outcome, |_| panic!("hook error"));

        assert_eq!(outcome.action as u8, HookAction::Replace as u8);
        let response = http_ctx
            .response
            .match_ref(Some, || None)
            .expect("response");
        assert_eq!(response.status.0, 200);
        assert_eq!(response.body.0.as_slice(), b"hello from example-noop");
    }

    #[test]
    fn tcp_callbacks_are_callable() {
        let module = build_tcp_module();
        let init_context = sample_init_context();
        let mut tcp_ctx = sample_tcp_context();

        assert!((module.init)(module.instance, &init_context as *const _).is_ok());

        assert_eq!(
            example_on_tcp_event(module.instance, &mut tcp_ctx as *mut _)
                .match_owned(|outcome| outcome, |_| panic!("hook error"))
                .action as u8,
            HookAction::Continue as u8
        );
        assert_eq!(
            example_on_tcp_event(module.instance, core::ptr::null_mut())
                .match_owned(|outcome| outcome, |_| panic!("hook error"))
                .action as u8,
            HookAction::Continue as u8
        );

        (module.shutdown)(module.instance);
    }

    #[test]
    fn udp_callbacks_are_callable() {
        let module = build_udp_module();
        let init_context = sample_init_context();
        let mut udp_ctx = sample_udp_context();

        assert!((module.init)(module.instance, &init_context as *const _).is_ok());

        assert_eq!(
            example_on_udp_event(module.instance, &mut udp_ctx as *mut _)
                .match_owned(|outcome| outcome, |_| panic!("hook error"))
                .action as u8,
            HookAction::Continue as u8
        );
        assert_eq!(
            example_on_udp_event(module.instance, core::ptr::null_mut())
                .match_owned(|outcome| outcome, |_| panic!("hook error"))
                .action as u8,
            HookAction::Continue as u8
        );

        (module.shutdown)(module.instance);
    }

    #[test]
    fn abi_decision_types_are_still_noop_shaped() {
        let decision = RouteDecision {
            upstream_to_set: Option::None(),
            action: RouteAction::Keep,
        };
        assert_eq!(decision.action as u8, RouteAction::Keep as u8);
        let hook = HookOutcome {
            action: HookAction::Continue,
        };
        assert_eq!(hook.action as u8, HookAction::Continue as u8);
    }
}
