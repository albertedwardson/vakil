//! Plugin library root module and protocol modules for stable Rust ABI.

use stabby::option::Option;
use stabby::result::Result;
use vakil_plugin_sys::{
    HookOutcome, HttpContext, PluginError, PluginInitContext, PluginManifest, TCPContext,
    UDPContext,
};

#[stabby::stabby]
pub struct PluginInstanceOpaque {
    _private: [u8; 0],
}
unsafe impl Send for PluginInstanceOpaque {}
unsafe impl Sync for PluginInstanceOpaque {}

/// ABI-stable plugin init callback.
///
/// The host passes an instance handle and a pointer to [`PluginInitContext`].
///
/// Failure semantics: if this function returns an error for a flow the host
/// treats the plugin chain as failed for that flow. Panics are not caught by
/// the host and may propagate.
pub type PluginInitFn = for<'a> extern "C" fn(
    *mut PluginInstanceOpaque,
    *const crate::PluginInitContext,
) -> Result<(), PluginError>;

/// ABI-stable HTTP phase callback.
///
/// Failure semantics: returning an `Err` or similar error indication causes the
/// host to consider the plugin chain failed for the current flow. Plugin
/// implementations MUST avoid panics — the host does not catch them.
///
/// Host local-reply behavior: when the plugin chain fails for a flow, the host
/// synthesizes a local-reply. If the plugin populated `HttpContext.response`
/// the host will use it as the local reply. Otherwise the host will use its
/// configured fallback response.
pub type HttpHookFn = extern "C" fn(
    *mut PluginInstanceOpaque,
    *mut crate::HttpContext,
) -> Result<HookOutcome, PluginError>;

/// same but for tcp
pub type TcpHookFn = extern "C" fn(
    *mut PluginInstanceOpaque,
    *mut crate::TCPContext,
) -> Result<HookOutcome, PluginError>;

/// same but for udp
pub type UdpHookFn = extern "C" fn(
    *mut PluginInstanceOpaque,
    *mut crate::UDPContext,
) -> Result<HookOutcome, PluginError>;

/// ABI-stable HTTP module factory callback.
pub type PluginHttpFactoryFn = extern "C" fn() -> Result<PluginHttpModule, PluginError>;

/// ABI-stable TCP module factory callback.
pub type PluginTcpFactoryFn = extern "C" fn() -> Result<PluginTcpModule, PluginError>;

/// ABI-stable UDP module factory callback.
pub type PluginUdpFactoryFn = extern "C" fn() -> Result<PluginUdpModule, PluginError>;

/// Shutdown callback.
pub type ShutdownFn = extern "C" fn(*mut PluginInstanceOpaque) -> Result<(), PluginError>;

/// Priority of execution for plugins. Lower priorities executed first
pub type Priority = u8;

#[stabby::stabby]
#[derive(Debug, Clone)]
pub struct PluginTcpModule {
    pub instance: *mut PluginInstanceOpaque,
    pub priority: Priority,
    pub init: PluginInitFn,
    pub shutdown: ShutdownFn,
    /// Route callback for TCP flows.
    pub on_route: Option<TcpHookFn>,
    /// TCP connect callback.
    pub on_connect: Option<TcpHookFn>,
    /// TCP data callback.
    pub on_data: Option<TcpHookFn>,
    /// TCP half-close callback.
    pub on_half_close: Option<TcpHookFn>,
    /// TCP close callback.
    pub on_close: Option<TcpHookFn>,
}

#[stabby::stabby]
#[derive(Debug, Clone)]
pub struct PluginUdpModule {
    pub instance: *mut PluginInstanceOpaque,
    pub priority: Priority,
    pub init: PluginInitFn,
    pub shutdown: ShutdownFn,
    /// Route callback for UDP flows.
    pub on_route: Option<UdpHookFn>,
    pub on_datagram: Option<UdpHookFn>,
    pub on_session_start: Option<UdpHookFn>,
    pub on_session_end: Option<UdpHookFn>,
}

#[stabby::stabby]
#[derive(Debug, Clone)]
pub struct PluginHttpModule {
    pub instance: *mut PluginInstanceOpaque,
    pub priority: Priority,
    pub init: PluginInitFn,
    pub shutdown: ShutdownFn,
    pub on_route: Option<HttpHookFn>,
    /// Request headers phase callback.
    pub on_request_headers: Option<HttpHookFn>,
    /// Request body phase callback.
    pub on_request_body: Option<HttpHookFn>,
    /// Response headers phase callback.
    pub on_response_headers: Option<HttpHookFn>,
    /// Response body phase callback.
    pub on_response_body: Option<HttpHookFn>,
    /// Trailers phase callback.
    pub on_trailers: Option<HttpHookFn>,
    /// Local reply callback for host-generated responses.
    pub on_local_reply: Option<HttpHookFn>,
}

// Default no-op implementations used by `Default` impls below.
extern "C" fn __vakil_default_init(
    _instance: *mut PluginInstanceOpaque,
    _ctx: *const PluginInitContext,
) -> Result<(), PluginError> {
    Result::Ok(())
}

extern "C" fn __vakil_default_shutdown(
    _instance: *mut PluginInstanceOpaque,
) -> Result<(), PluginError> {
    Result::Ok(())
}

impl Default for PluginTcpModule {
    fn default() -> Self {
        PluginTcpModule {
            instance: std::ptr::null_mut(),
            priority: Default::default(),
            init: __vakil_default_init,
            shutdown: __vakil_default_shutdown,
            on_route: Default::default(),
            on_connect: Default::default(),
            on_data: Default::default(),
            on_half_close: Default::default(),
            on_close: Default::default(),
        }
    }
}

impl Default for PluginUdpModule {
    fn default() -> Self {
        PluginUdpModule {
            instance: std::ptr::null_mut(),
            priority: Default::default(),
            init: __vakil_default_init,
            shutdown: __vakil_default_shutdown,
            on_route: Default::default(),
            on_datagram: Default::default(),
            on_session_start: Default::default(),
            on_session_end: Default::default(),
        }
    }
}

impl Default for PluginHttpModule {
    fn default() -> Self {
        PluginHttpModule {
            instance: std::ptr::null_mut(),
            priority: Default::default(),
            init: __vakil_default_init,
            shutdown: __vakil_default_shutdown,
            on_route: Default::default(),
            on_request_headers: Default::default(),
            on_request_body: Default::default(),
            on_response_headers: Default::default(),
            on_response_body: Default::default(),
            on_trailers: Default::default(),
            on_local_reply: Default::default(),
        }
    }
}

/// Root module exported by a plugin library.
#[stabby::stabby]
#[derive(Clone, Debug)]
pub struct PluginRootModule {
    /// Plugin manifest used for compatibility and policy checks.
    pub manifest: PluginManifest,
    /// Factory for optional HTTP module; `None` means HTTP is unsupported.
    pub create_http: Option<PluginHttpFactoryFn>,
    /// Factory for optional TCP module; `None` means TCP is unsupported.
    pub create_tcp: Option<PluginTcpFactoryFn>,
    /// Factory for optional UDP module; `None` means UDP is unsupported.
    pub create_udp: Option<PluginUdpFactoryFn>,
}

pub trait PluginModuleTrait {
    fn init(&mut self, ctx: *const PluginInitContext) -> Result<(), PluginError>;
    fn shutdown(&mut self) -> Result<(), PluginError>;
}

impl PluginModuleTrait for PluginTcpModule {
    fn init(&mut self, ctx: *const PluginInitContext) -> Result<(), PluginError> {
        (self.init)(self.instance, ctx)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        (self.shutdown)(self.instance)
    }
}
impl PluginModuleTrait for PluginUdpModule {
    fn init(&mut self, ctx: *const PluginInitContext) -> Result<(), PluginError> {
        (self.init)(self.instance, ctx)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        (self.shutdown)(self.instance)
    }
}

impl PluginModuleTrait for PluginHttpModule {
    fn init(&mut self, ctx: *const PluginInitContext) -> Result<(), PluginError> {
        (self.init)(self.instance, ctx)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        (self.shutdown)(self.instance)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PluginHttpModule, PluginInstanceOpaque, PluginManifest, PluginModuleTrait,
        PluginRootModule, PluginTcpModule, PluginUdpModule,
    };
    use stabby::option::Option as AbiOption;
    use stabby::result::Result;
    use std::ptr;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static INIT_COUNT: AtomicUsize = AtomicUsize::new(0);
    static SHUTDOWN_COUNT: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn test_init(
        _instance: *mut PluginInstanceOpaque,
        _ctx: *const vakil_plugin_sys::PluginInitContext,
    ) -> Result<(), vakil_plugin_sys::PluginError> {
        INIT_COUNT.fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn test_shutdown(
        _instance: *mut PluginInstanceOpaque,
    ) -> Result<(), vakil_plugin_sys::PluginError> {
        SHUTDOWN_COUNT.fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn create_http() -> Result<PluginHttpModule, vakil_plugin_sys::PluginError> {
        Result::Ok(PluginHttpModule::default())
    }

    fn assert_default_module_fields<T>(module: &T)
    where
        T: DefaultModuleAssertions,
    {
        module.assert_default_fields();
    }

    trait DefaultModuleAssertions {
        fn assert_default_fields(&self);
    }

    impl DefaultModuleAssertions for PluginTcpModule {
        fn assert_default_fields(&self) {
            assert!(self.instance.is_null());
            assert_eq!(self.priority, 0);
            assert!(self.on_route.is_none());
            assert!(self.on_connect.is_none());
            assert!(self.on_data.is_none());
            assert!(self.on_half_close.is_none());
            assert!(self.on_close.is_none());
        }
    }

    impl DefaultModuleAssertions for PluginUdpModule {
        fn assert_default_fields(&self) {
            assert!(self.instance.is_null());
            assert_eq!(self.priority, 0);
            assert!(self.on_route.is_none());
            assert!(self.on_datagram.is_none());
            assert!(self.on_session_start.is_none());
            assert!(self.on_session_end.is_none());
        }
    }

    impl DefaultModuleAssertions for PluginHttpModule {
        fn assert_default_fields(&self) {
            assert!(self.instance.is_null());
            assert_eq!(self.priority, 0);
            assert!(self.on_route.is_none());
            assert!(self.on_request_headers.is_none());
            assert!(self.on_request_body.is_none());
            assert!(self.on_response_headers.is_none());
            assert!(self.on_response_body.is_none());
            assert!(self.on_trailers.is_none());
            assert!(self.on_local_reply.is_none());
        }
    }

    #[test]
    fn default_modules_start_empty() {
        assert_default_module_fields(&PluginTcpModule::default());
        assert_default_module_fields(&PluginUdpModule::default());
        assert_default_module_fields(&PluginHttpModule::default());
    }

    #[test]
    fn module_trait_methods_invoke_callbacks() {
        INIT_COUNT.store(0, Ordering::SeqCst);
        SHUTDOWN_COUNT.store(0, Ordering::SeqCst);

        let mut tcp = PluginTcpModule {
            instance: ptr::null_mut(),
            priority: 1,
            init: test_init,
            shutdown: test_shutdown,
            on_route: AbiOption::None(),
            on_connect: AbiOption::None(),
            on_data: AbiOption::None(),
            on_half_close: AbiOption::None(),
            on_close: AbiOption::None(),
        };
        let mut udp = PluginUdpModule {
            instance: ptr::null_mut(),
            priority: 2,
            init: test_init,
            shutdown: test_shutdown,
            on_route: AbiOption::None(),
            on_datagram: AbiOption::None(),
            on_session_start: AbiOption::None(),
            on_session_end: AbiOption::None(),
        };
        let mut http = PluginHttpModule {
            instance: ptr::null_mut(),
            priority: 3,
            init: test_init,
            shutdown: test_shutdown,
            on_route: AbiOption::None(),
            on_request_headers: AbiOption::None(),
            on_request_body: AbiOption::None(),
            on_response_headers: AbiOption::None(),
            on_response_body: AbiOption::None(),
            on_trailers: AbiOption::None(),
            on_local_reply: AbiOption::None(),
        };

        assert!(tcp.init(ptr::null()).is_ok());
        assert!(udp.init(ptr::null()).is_ok());
        assert!(http.init(ptr::null()).is_ok());

        assert!(tcp.shutdown().is_ok());
        assert!(udp.shutdown().is_ok());
        assert!(http.shutdown().is_ok());

        assert_eq!(INIT_COUNT.load(Ordering::SeqCst), 3);
        assert_eq!(SHUTDOWN_COUNT.load(Ordering::SeqCst), 3);

        let root = PluginRootModule {
            manifest: PluginManifest {
                name: "example".to_string().into(),
                version: vakil_plugin_sys::SemVer {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
            },
            create_http: AbiOption::Some(create_http),
            create_tcp: AbiOption::None(),
            create_udp: AbiOption::None(),
        };

        assert_eq!(root.manifest.name.as_str(), "example");
        assert!(root.create_http.is_some());
        assert!(root.create_tcp.is_none());
        assert!(root.create_udp.is_none());
    }
}
