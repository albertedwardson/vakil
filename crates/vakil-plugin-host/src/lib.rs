//! Plugin host loader using stabby for stable Rust ABI.

mod version;

use crate::version::HOST_ABI_VERSION;
use anyhow::{Result, bail};
use libloading::Library;
use log::{debug, info};
use stabby::option::Option as AbiOption;
use stabby::string::String as AbiString;
use stabby::vec::Vec;
use std::path::Path;
use vakil_plugin_api::{PluginHttpModule, PluginRootModule, PluginTcpModule, PluginUdpModule};
use vakil_plugin_sys::{EnvSnapshot, KVPair, PluginInitContext, PluginManifest, SemVer};

/// Loaded plugin instance with its exported root module and optional protocol
/// modules.
///
/// IMPORTANT: `library` must be dropped *after* any plugin-owned data (`root`,
/// `modules`). Plugin-owned allocations (stabby strings/vecs) may run
/// destructor code that lives in the plugin library. Dropping `library` first
/// causes `dlclose` while those destructors still run -> use-after-free ->
/// SIGSEGV. Keep `library` last so it is dropped after other fields.
#[derive(Debug)]
pub struct LoadedPlugin {
    /// Dynamic library handle kept alive for symbol validity.
    /// Root module exported by plugin (`get_library`).
    pub root: PluginRootModule,
    /// Instantiated protocol modules owned by the host.
    pub modules: LoadedPluginModules,
    /// Dynamic library handle kept alive for symbol validity.
    pub library: Library,
}

/// Instantiated protocol modules loaded from a single plugin root.
#[derive(Default, Clone, Debug)]
pub struct LoadedPluginModules {
    /// Optional HTTP protocol module.
    pub http: Option<PluginHttpModule>,
    /// Optional TCP protocol module.
    pub tcp: Option<PluginTcpModule>,
    /// Optional UDP protocol module.
    pub udp: Option<PluginUdpModule>,
}

impl LoadedPluginModules {
    /// Shut down any instantiated protocol modules.
    pub fn shutdown(&mut self) {
        if let Some(module) = self.http.take() {
            (module.shutdown)(module.instance);
        }

        if let Some(module) = self.tcp.take() {
            (module.shutdown)(module.instance);
        }

        if let Some(module) = self.udp.take() {
            (module.shutdown)(module.instance);
        }
    }
}

impl LoadedPlugin {
    /// Load a plugin root module from a file path and run startup init for
    /// supported modules.
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref();
        info!("loading plugin library {}", path.display());
        let library = unsafe { Library::new(path) }
            .map_err(|e| anyhow::anyhow!("failed to open plugin library: {}", e))?;

        let get_library = unsafe {
            library
                .get::<extern "C" fn() -> PluginRootModule>(b"get_library")
                .map_err(|e| anyhow::anyhow!("failed to load plugin root symbol: {}", e))?
        };

        let root = get_library();
        validate_manifest(&root.manifest)?;

        let init_context = build_init_context(path, std::env::vars())?;
        let modules = load_modules(&root, &init_context)?;

        info!(
            "loaded plugin {} v{}.{}.{}",
            root.manifest.name.as_str(),
            root.manifest.version.major,
            root.manifest.version.minor,
            root.manifest.version.patch
        );

        Ok(Self {
            library,
            root,
            modules,
        })
    }

    /// Return plugin manifest name.
    pub fn name(&self) -> &str {
        self.root.manifest.name.as_str()
    }

    /// Return whether the plugin exposes an HTTP module.
    pub fn has_http(&self) -> bool {
        self.modules.http.is_some()
    }

    /// Return whether the plugin exposes a TCP module.
    pub fn has_tcp(&self) -> bool {
        self.modules.tcp.is_some()
    }

    /// Return whether the plugin exposes a UDP module.
    pub fn has_udp(&self) -> bool {
        self.modules.udp.is_some()
    }

    /// Call shutdown on all instantiated protocol modules.
    pub fn shutdown(&mut self) {
        self.modules.shutdown();
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Build a plugin init context from startup metadata and environment values.
pub fn build_init_context<I, K, V>(path: &Path, vars: I) -> Result<PluginInitContext>
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let plugin_dir = path
        .parent()
        .map(|dir| AbiString::from(dir.to_string_lossy().as_ref()));
    let env = collect_env_snapshot(vars);

    Ok(PluginInitContext {
        library_path: AbiString::from(path.to_string_lossy().as_ref()),
        plugin_dir: match plugin_dir {
            Some(dir) => AbiOption::Some(dir),
            None => AbiOption::None(),
        },
        host_version: SemVer::from_string(String::from(env!("CARGO_PKG_VERSION"))),
        env,
    })
}

macro_rules! load_block {
    ($modules:ident, $field_name:ident, $create_opt:expr, $ctx:expr) => {
        let field_name_str = stringify!($field_name);
        if let Some(factory) = $create_opt.as_ref() {
            debug!("initializing {} module", field_name_str);
            if let Some(module) = factory().ok() {
                let init_res = (module.init)(module.instance, $ctx as *const _);
                if init_res.is_ok() {
                    info!("initialized {} module", field_name_str);
                    $modules.$field_name = Some(module);
                } else {
                    return Err(anyhow::anyhow!("{} module init failed", field_name_str));
                }
            } else {
                return Err(anyhow::anyhow!("{} module factory failed", field_name_str));
            }
        }
    };
}

/// Load and initialize all protocol modules exposed by a plugin root.
pub fn load_modules(
    root: &PluginRootModule,
    init_context: &PluginInitContext,
) -> Result<LoadedPluginModules> {
    let mut modules = LoadedPluginModules::default();
    load_block!(modules, http, root.create_http, init_context);
    load_block!(modules, tcp, root.create_tcp, init_context);
    load_block!(modules, udp, root.create_udp, init_context);
    Ok(modules)
}

fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    if manifest.version.major != HOST_ABI_VERSION.major {
        bail!(
            "plugin major version {} is incompatible with host ABI {}",
            manifest.version.major,
            HOST_ABI_VERSION.major
        );
    }

    if manifest.version.minor > HOST_ABI_VERSION.minor {
        bail!(
            "plugin minor version {} is newer than host ABI {}",
            manifest.version.minor,
            HOST_ABI_VERSION.minor
        );
    }

    Ok(())
}

fn collect_env_snapshot<I, K, V>(vars: I) -> EnvSnapshot
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: AsRef<str>,
{
    let mut entries = Vec::new();

    for (name, value) in vars {
        entries.push(KVPair {
            name: AbiString::from(name.as_ref()),
            value: AbiString::from(value.as_ref()),
        });
    }

    EnvSnapshot { entries }
}

#[cfg(test)]
mod tests {
    use super::{
        HOST_ABI_VERSION, build_init_context, collect_env_snapshot, load_modules, validate_manifest,
    };
    use stabby::option::Option;
    use stabby::result::Result;
    use stabby::string::String as AbiString;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, OnceLock};
    use vakil_plugin_api::{
        PluginHttpModule, PluginInstanceOpaque, PluginRootModule, PluginTcpModule, PluginUdpModule,
    };
    use vakil_plugin_sys::{
        HookAction, HookOutcome, HttpContext, PluginError, PluginInitContext, PluginManifest,
        SemVer, TCPContext, UDPContext,
    };

    static INIT_COUNTS: OnceLock<Arc<[AtomicUsize; 3]>> = OnceLock::new();
    static SHUTDOWN_COUNTS: OnceLock<Arc<[AtomicUsize; 3]>> = OnceLock::new();

    fn counters() -> Arc<[AtomicUsize; 3]> {
        INIT_COUNTS
            .get_or_init(|| {
                Arc::new([
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                ])
            })
            .clone()
    }

    fn shutdown_counters() -> Arc<[AtomicUsize; 3]> {
        SHUTDOWN_COUNTS
            .get_or_init(|| {
                Arc::new([
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                    AtomicUsize::new(0),
                ])
            })
            .clone()
    }

    extern "C" fn test_init(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *const PluginInitContext,
    ) -> Result<(), PluginError> {
        counters()[0].fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn test_tcp_init(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *const PluginInitContext,
    ) -> Result<(), PluginError> {
        counters()[1].fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn test_udp_init(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *const PluginInitContext,
    ) -> Result<(), PluginError> {
        counters()[2].fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn test_shutdown_http(_inst: *mut PluginInstanceOpaque) -> Result<(), PluginError> {
        shutdown_counters()[0].fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn test_shutdown_tcp(_inst: *mut PluginInstanceOpaque) -> Result<(), PluginError> {
        shutdown_counters()[1].fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    extern "C" fn test_shutdown_udp(_inst: *mut PluginInstanceOpaque) -> Result<(), PluginError> {
        shutdown_counters()[2].fetch_add(1, Ordering::SeqCst);
        Result::Ok(())
    }

    fn http_module() -> PluginHttpModule {
        PluginHttpModule {
            init: test_init,
            on_route: Option::Some(test_route),
            on_request_headers: Option::Some(test_http_hook),
            on_request_body: Option::Some(test_http_hook),
            on_response_headers: Option::Some(test_http_hook),
            on_response_body: Option::Some(test_http_hook),
            on_trailers: Option::Some(test_http_hook),
            on_local_reply: Option::Some(test_http_hook),
            shutdown: test_shutdown_http,
            ..Default::default()
        }
    }

    fn tcp_module() -> PluginTcpModule {
        PluginTcpModule {
            init: test_tcp_init,
            on_connect: Option::Some(test_tcp_hook),
            on_data: Option::Some(test_tcp_hook),
            on_half_close: Option::Some(test_tcp_hook),
            on_close: Option::Some(test_tcp_hook),
            shutdown: test_shutdown_tcp,
            ..Default::default()
        }
    }

    fn udp_module() -> PluginUdpModule {
        PluginUdpModule {
            init: test_udp_init,
            on_datagram: Option::Some(test_udp_hook),
            on_session_start: Option::Some(test_udp_hook),
            on_session_end: Option::Some(test_udp_hook),
            shutdown: test_shutdown_udp,
            ..Default::default()
        }
    }

    extern "C" fn test_route(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *mut HttpContext,
    ) -> Result<HookOutcome, PluginError> {
        Result::Ok(HookOutcome::default())
    }

    extern "C" fn test_http_hook(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *mut HttpContext,
    ) -> Result<HookOutcome, PluginError> {
        Result::Ok(HookOutcome {
            action: HookAction::Continue,
        })
    }

    extern "C" fn test_tcp_hook(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *mut TCPContext,
    ) -> Result<HookOutcome, PluginError> {
        Result::Ok(HookOutcome {
            action: HookAction::Continue,
        })
    }

    extern "C" fn test_udp_hook(
        _inst: *mut PluginInstanceOpaque,
        _ctx: *mut UDPContext,
    ) -> Result<HookOutcome, PluginError> {
        Result::Ok(HookOutcome {
            action: HookAction::Continue,
        })
    }

    fn valid_root() -> PluginRootModule {
        PluginRootModule {
            manifest: PluginManifest {
                name: AbiString::from("test-plugin"),
                version: HOST_ABI_VERSION,
            },
            create_http: Option::Some(create_http),
            create_tcp: Option::Some(create_tcp),
            create_udp: Option::Some(create_udp),
        }
    }

    extern "C" fn create_http() -> Result<PluginHttpModule, PluginError> {
        Result::Ok(http_module())
    }

    extern "C" fn create_tcp() -> Result<PluginTcpModule, PluginError> {
        Result::Ok(tcp_module())
    }

    extern "C" fn create_udp() -> Result<PluginUdpModule, PluginError> {
        Result::Ok(udp_module())
    }

    #[test]
    fn builds_init_context_with_metadata() {
        let context = build_init_context(
            PathBuf::from("/tmp/plugins/example.so").as_path(),
            [("A", "1"), ("B", "2")],
        )
        .expect("init context");

        assert_eq!(context.library_path.as_str(), "/tmp/plugins/example.so");
        assert_eq!(
            context.plugin_dir.as_ref().expect("plugin dir").as_str(),
            "/tmp/plugins"
        );
        assert_eq!(context.env.entries.len(), 2);
        let version_str = format!(
            "{}.{}.{}",
            context.host_version.major, context.host_version.minor, context.host_version.patch
        );
        assert_eq!(version_str, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn builds_init_context_without_parent_directory() {
        let context = build_init_context(
            PathBuf::from("/").as_path(),
            std::iter::empty::<(&str, &str)>(),
        )
        .expect("init context");

        assert_eq!(context.library_path.as_str(), "/");
        assert!(context.plugin_dir.is_none());
    }

    #[test]
    fn validates_manifest() {
        let root = valid_root();
        validate_manifest(&root.manifest).expect("valid root");
    }

    #[test]
    fn loads_and_shuts_down_all_protocol_modules() {
        let root = valid_root();
        let context = build_init_context(
            PathBuf::from("/tmp/plugins/example.so").as_path(),
            std::iter::empty::<(&str, &str)>(),
        )
        .expect("init context");

        let init_before = [
            counters()[0].load(Ordering::SeqCst),
            counters()[1].load(Ordering::SeqCst),
            counters()[2].load(Ordering::SeqCst),
        ];
        let shutdown_before = [
            shutdown_counters()[0].load(Ordering::SeqCst),
            shutdown_counters()[1].load(Ordering::SeqCst),
            shutdown_counters()[2].load(Ordering::SeqCst),
        ];

        let mut modules = load_modules(&root, &context).expect("modules");
        assert!(modules.http.is_some());
        assert!(modules.tcp.is_some());
        assert!(modules.udp.is_some());
        assert_eq!(counters()[0].load(Ordering::SeqCst) - init_before[0], 1);
        assert_eq!(counters()[1].load(Ordering::SeqCst) - init_before[1], 1);
        assert_eq!(counters()[2].load(Ordering::SeqCst) - init_before[2], 1);

        modules.shutdown();
        assert_eq!(
            shutdown_counters()[0].load(Ordering::SeqCst) - shutdown_before[0],
            1
        );
        assert_eq!(
            shutdown_counters()[1].load(Ordering::SeqCst) - shutdown_before[1],
            1
        );
        assert_eq!(
            shutdown_counters()[2].load(Ordering::SeqCst) - shutdown_before[2],
            1
        );
    }

    #[test]
    fn rejects_incompatible_manifest_versions() {
        let mut root = valid_root();
        root.manifest.version = SemVer {
            major: HOST_ABI_VERSION.major + 1,
            minor: 0,
            patch: 0,
        };

        match validate_manifest(&root.manifest) {
            Ok(()) => panic!("version mismatch should fail"),
            Err(e) => assert!(e.to_string().contains("incompatible")),
        }
    }

    #[test]
    fn collects_env_snapshot() {
        let snapshot = collect_env_snapshot([("X", "y")]);
        assert_eq!(snapshot.entries.len(), 1);
        assert_eq!(snapshot.entries[0].name.as_str(), "X");
        assert_eq!(snapshot.entries[0].value.as_str(), "y");
    }
}
