use std::hash::BuildHasherDefault;
use xxhash_rust::xxh3::Xxh3;
type XxBuildHasher = BuildHasherDefault<xxhash_rust::xxh3::Xxh3>;
use serde::Deserialize;
use stabby::option::Option as AbiOption;
use stabby::result::Result as AbiResult;
use stabby::string::String as AbiString;
use std::collections::HashMap;
use std::fs;
use std::hash::Hash;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vakil_plugin_api::{PluginInstanceOpaque, PluginRootModule, PluginTcpModule, PluginUdpModule};
use vakil_plugin_sys::{
    HookAction, HookOutcome, PluginError, PluginInitContext, PluginManifest, RouteAction,
    RouteDecision, SemVer, SocketAddress, TCPContext, UDPContext,
};
const CONFIG_ENV: &str = "VAKIL_L4_STICKY_CONFIG";

#[derive(Debug, Deserialize)]
struct FileConfig {
    #[serde(default = "default_ttl_secs")]
    ttl_secs: u64,
    #[serde(default)]
    fallback_backends: Vec<String>,
    #[serde(default)]
    listeners: Vec<ListenerConfig>,
}

#[derive(Debug, Deserialize)]
struct ListenerConfig {
    name: String,
    backends: Vec<String>,
}

#[derive(Debug, Clone)]
struct LoadedConfig {
    ttl: Duration,
    fallback_backends: Vec<String>,
    listener_backends: HashMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
struct SharedInitState {
    state: Option<Arc<SharedState>>,
    init_error: Option<String>,
}

#[derive(Debug)]
struct SharedState {
    config: LoadedConfig,
    assignments: Mutex<HashMap<StickyKey, StickyEntry, XxBuildHasher>>,
}

#[derive(Debug, Clone)]
struct StickyEntry {
    backend: String,
    expires_at: Instant,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct StickyKey {
    listener: String,
    client_ip: IpAddr,
}

#[derive(Debug)]
struct ModuleHandle {
    shared: Arc<Mutex<SharedInitState>>,
}

static GLOBAL_SHARED_INIT: std::sync::OnceLock<Arc<Mutex<SharedInitState>>> =
    std::sync::OnceLock::new();

extern "C" fn sticky_init(
    inst: *mut PluginInstanceOpaque,
    ctx: *const PluginInitContext,
) -> AbiResult<(), PluginError> {
    let Some(module) = module_ref(inst) else {
        return AbiResult::Err(plugin_error("missing module handle"));
    };

    let Some(ctx) = (unsafe { ctx.as_ref() }) else {
        return AbiResult::Err(plugin_error("missing init context"));
    };

    match ensure_state(&module.shared, ctx) {
        Ok(_) => AbiResult::Ok(()),
        Err(err) => AbiResult::Err(err),
    }
}

// extern "C" fn sticky_on_route(
//     inst: *mut PluginInstanceOpaque,
//     ctx: *mut HttpContext,
// ) -> AbiResult<RouteDecision, PluginError> {
//     let Some(module) = module_ref(inst) else {
//         return AbiResult::Ok(keep_route());
//     };
//     let Some(ctx) = (unsafe { ctx.as_mut() }) else {
//         return AbiResult::Ok(keep_route());
//     };

//     let Some(state) = current_state(&module.shared) else {
//         return AbiResult::Ok(keep_route());
//     };

//     let Some(client_ip) = parse_client_ip(ctx.peer.as_str()) else {
//         return AbiResult::Ok(keep_route());
//     };

//     let backend = match state.select_backend(ctx.listener.as_str(),
// client_ip) {         Ok(backend) => backend,
//         Err(err) => {
//             warn!(
//                 "[example-l4-sticky] route error: {}",
//                 plugin_error_message(&err)
//             );
//             return AbiResult::Ok(reject_route(503, "sticky backend
// unavailable"));         }
//     };

//     info!(
//         "[example-l4-sticky] listener={} client_ip={} backend={}",
//         ctx.listener.as_str(),
//         client_ip,
//         backend
//     );

//     AbiResult::Ok(route_to(backend))
// }

extern "C" fn sticky_on_tcp_connect(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut TCPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_on_tcp_data(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut TCPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_on_tcp_close(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut TCPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_on_tcp_half_close(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut TCPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_on_udp_datagram(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut UDPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_on_udp_session_start(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut UDPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_on_udp_session_end(
    _inst: *mut PluginInstanceOpaque,
    ctx: *mut UDPContext,
) -> AbiResult<HookOutcome, PluginError> {
    let _ = unsafe { ctx.as_mut() };
    AbiResult::Ok(continue_hook())
}

extern "C" fn sticky_shutdown(inst: *mut PluginInstanceOpaque) -> AbiResult<(), PluginError> {
    if inst.is_null() {
        return AbiResult::Ok(());
    }

    unsafe {
        drop(Box::from_raw(inst as *mut ModuleHandle));
    }

    AbiResult::Ok(())
}

#[inline]
fn continue_hook() -> HookOutcome {
    HookOutcome {
        action: HookAction::Continue,
    }
}

#[inline]
fn keep_route() -> RouteDecision {
    RouteDecision {
        upstream_to_set: AbiOption::None(),
        action: RouteAction::Keep,
    }
}

#[inline]
fn route_to(backend: String) -> RouteDecision {
    let Ok(upstream) = backend_to_socket_address(backend.as_str()) else {
        return keep_route();
    };

    RouteDecision {
        upstream_to_set: AbiOption::Some(upstream),
        action: RouteAction::ReplaceUpstream,
    }
}

#[inline]
fn reject_route(_status: u16, _message: &str) -> RouteDecision {
    RouteDecision {
        upstream_to_set: AbiOption::None(),
        action: RouteAction::Reject,
    }
}

fn backend_to_socket_address(value: &str) -> Result<SocketAddress, PluginError> {
    let Some((host, port)) = value.rsplit_once(':') else {
        return Err(plugin_error(format!("invalid backend address {}", value)));
    };

    let Ok(port) = port.parse::<u16>() else {
        return Err(plugin_error(format!("invalid backend port {}", value)));
    };

    Ok(SocketAddress {
        host: AbiString::from(host),
        port,
    })
}

#[inline]
fn plugin_error(message: impl Into<String>) -> PluginError {
    PluginError {
        message: AbiOption::Some(AbiString::from(message.into().as_str())),
    }
}

#[inline]
fn plugin_error_message(err: &PluginError) -> String {
    err.message
        .as_ref()
        .map(|message| message.as_str().to_string())
        .unwrap_or_else(|| "unknown plugin error".to_string())
}

fn module_ref(inst: *mut PluginInstanceOpaque) -> Option<&'static ModuleHandle> {
    if inst.is_null() {
        return None;
    }

    unsafe { (inst as *const ModuleHandle).as_ref() }
}

fn module_instance(shared: Arc<Mutex<SharedInitState>>) -> *mut PluginInstanceOpaque {
    Box::into_raw(Box::new(ModuleHandle { shared })) as *mut PluginInstanceOpaque
}

fn shared_init_slot() -> Arc<Mutex<SharedInitState>> {
    GLOBAL_SHARED_INIT
        .get_or_init(|| Arc::new(Mutex::new(SharedInitState::default())))
        .clone()
}

fn ensure_state(
    slot: &Arc<Mutex<SharedInitState>>,
    ctx: &PluginInitContext,
) -> Result<Arc<SharedState>, PluginError> {
    let mut guard = slot.lock().expect("shared init state poisoned");

    if let Some(state) = guard.state.as_ref() {
        return Ok(state.clone());
    }

    if let Some(error) = guard.init_error.as_ref() {
        return Err(plugin_error(error.clone()));
    }

    match load_state(ctx) {
        Ok(state) => {
            guard.state = Some(state.clone());
            Ok(state)
        }
        Err(err) => {
            guard.init_error = err
                .message
                .as_ref()
                .map(|message| message.as_str().to_string())
                .or_else(|| Some("failed to load sticky config".to_string()));
            Err(err)
        }
    }
}

#[inline]
fn current_state(slot: &Arc<Mutex<SharedInitState>>) -> Option<Arc<SharedState>> {
    slot.lock().ok()?.state.as_ref().cloned()
}

fn load_state(ctx: &PluginInitContext) -> Result<Arc<SharedState>, PluginError> {
    let config_path = resolve_config_path(ctx);
    let config_text = fs::read_to_string(&config_path).map_err(|error| {
        plugin_error(format!(
            "failed to read sticky config {}: {}",
            config_path.display(),
            error
        ))
    })?;

    let file_config: FileConfig = toml::from_str(&config_text).map_err(|error| {
        plugin_error(format!(
            "failed to parse sticky config {}: {}",
            config_path.display(),
            error
        ))
    })?;

    let config = LoadedConfig::try_from(file_config)?;
    Ok(Arc::new(SharedState::new(config)))
}

fn resolve_config_path(ctx: &PluginInitContext) -> PathBuf {
    if let Some(path) = env_value(ctx, CONFIG_ENV) {
        return PathBuf::from(path);
    }

    if let Some(plugin_dir) = ctx.plugin_dir.as_ref() {
        return Path::new(plugin_dir.as_str()).join("sticky-l4.toml");
    }

    PathBuf::from("sticky-l4.toml")
}

fn env_value<'a>(ctx: &'a PluginInitContext, name: &str) -> Option<&'a str> {
    ctx.env
        .entries
        .iter()
        .find(|entry| entry.name.as_str() == name)
        .map(|entry| entry.value.as_str())
}

#[inline]
fn default_ttl_secs() -> u64 {
    900
}

#[inline]
fn normalize(value: &str, field: &str) -> Result<String, PluginError> {
    let trimmed = value.trim();

    if trimmed.is_empty() {
        return Err(plugin_error(format!("{} cannot be empty", field)));
    }

    Ok(trimmed.to_string())
}

impl SharedState {
    fn new(config: LoadedConfig) -> Self {
        Self {
            config,
            assignments: Mutex::new(HashMap::with_hasher(XxBuildHasher::default())),
        }
    }

    fn select_backend(&self, listener: &str, client_ip: IpAddr) -> Result<String, PluginError> {
        let pool = self.config.pool_for_listener(listener);

        if pool.is_empty() {
            return Err(plugin_error(format!(
                "no backends configured for listener {}",
                listener
            )));
        }

        let key = StickyKey {
            listener: listener.to_string(),
            client_ip,
        };
        let now = Instant::now();

        let mut assignments = self
            .assignments
            .lock()
            .expect("sticky assignment cache poisoned");

        assignments.retain(|_, entry| entry.expires_at > now);

        if let Some(entry) = assignments.get(&key)
            && pool.iter().any(|backend| backend == &entry.backend)
        {
            return Ok(entry.backend.clone());
        }

        let backend = choose_backend(listener, client_ip, pool);
        assignments.insert(
            key,
            StickyEntry {
                backend: backend.clone(),
                expires_at: now + self.config.ttl,
            },
        );

        Ok(backend)
    }
}

impl LoadedConfig {
    fn pool_for_listener(&self, listener: &str) -> &[String] {
        self.listener_backends
            .get(listener)
            .map(|backends| backends.as_slice())
            .unwrap_or_else(|| self.fallback_backends.as_slice())
    }
}

impl TryFrom<FileConfig> for LoadedConfig {
    type Error = PluginError;

    fn try_from(value: FileConfig) -> Result<Self, Self::Error> {
        if value.ttl_secs == 0 {
            return Err(plugin_error("ttl_secs must be greater than zero"));
        }

        let mut listener_backends = HashMap::new();

        for listener in value.listeners {
            let name = normalize(&listener.name, "listener name")?;

            if listener_backends.contains_key(&name) {
                return Err(plugin_error(format!("duplicate listener {}", name)));
            }

            let backends = normalize_backends(&listener.backends, &format!("listener {}", name))?;
            listener_backends.insert(name, backends);
        }

        let fallback_backends = normalize_backends(&value.fallback_backends, "fallback_backends")?;

        if listener_backends.is_empty() && fallback_backends.is_empty() {
            return Err(plugin_error(
                "sticky config must define at least one listener pool or fallback backends",
            ));
        }

        Ok(Self {
            ttl: Duration::from_secs(value.ttl_secs),
            fallback_backends,
            listener_backends,
        })
    }
}

#[inline]
fn normalize_backends(values: &[String], field: &str) -> Result<Vec<String>, PluginError> {
    let mut backends = Vec::with_capacity(values.len());

    for backend in values {
        backends.push(normalize(backend, field)?);
    }

    Ok(backends)
}

#[inline]
fn parse_client_ip(peer: &str) -> Option<IpAddr> {
    peer.parse::<SocketAddr>().ok().map(|socket| socket.ip())
}

#[inline]
fn choose_backend(listener: &str, client_ip: IpAddr, pool: &[String]) -> String {
    let mut hasher = Xxh3::new();
    hasher.update(listener.as_bytes());
    match client_ip {
        IpAddr::V4(v4) => hasher.update(&v4.octets()),
        IpAddr::V6(v6) => hasher.update(&v6.octets()),
    }
    let index = (hasher.digest() as usize) % pool.len();
    pool[index].clone()
}

fn build_tcp_module() -> PluginTcpModule {
    let shared = shared_init_slot();
    PluginTcpModule {
        instance: module_instance(shared),
        priority: 0,
        init: sticky_init,
        on_route: Default::default(), // TODO
        on_connect: AbiOption::Some(sticky_on_tcp_connect),
        on_data: AbiOption::Some(sticky_on_tcp_data),
        on_half_close: AbiOption::Some(sticky_on_tcp_half_close),
        on_close: AbiOption::Some(sticky_on_tcp_close),
        shutdown: sticky_shutdown,
    }
}

fn build_udp_module() -> PluginUdpModule {
    let shared = shared_init_slot();
    PluginUdpModule {
        instance: module_instance(shared),
        priority: 0,
        init: sticky_init,
        on_route: Default::default(), // TODO
        on_datagram: AbiOption::Some(sticky_on_udp_datagram),
        on_session_start: AbiOption::Some(sticky_on_udp_session_start),
        on_session_end: AbiOption::Some(sticky_on_udp_session_end),
        shutdown: sticky_shutdown,
    }
}

extern "C" fn create_tcp() -> AbiResult<PluginTcpModule, PluginError> {
    AbiResult::Ok(build_tcp_module())
}

extern "C" fn create_udp() -> AbiResult<PluginUdpModule, PluginError> {
    AbiResult::Ok(build_udp_module())
}

#[unsafe(no_mangle)]
pub extern "C" fn get_library() -> PluginRootModule {
    PluginRootModule {
        manifest: PluginManifest {
            name: AbiString::from("example-l4-sticky"),
            version: SemVer {
                major: 0,
                minor: 1,
                patch: 0,
            },
        },
        create_http: AbiOption::None(),
        create_tcp: AbiOption::Some(create_tcp),
        create_udp: AbiOption::Some(create_udp),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AbiOption, AbiString, CONFIG_ENV, FileConfig, HookAction, LoadedConfig, SharedInitState,
        choose_backend, create_tcp, create_udp, default_ttl_secs, ensure_state, get_library,
        load_state, normalize_backends, parse_client_ip, plugin_error_message, resolve_config_path,
        sticky_init, sticky_shutdown,
    };
    use stabby::vec::Vec as AbiVec;
    use std::fs;
    use std::net::{IpAddr, Ipv4Addr};
    use std::path::{Path, PathBuf};
    use std::ptr;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use vakil_plugin_sys::{EnvSnapshot, KVPair, PluginInitContext, SemVer};

    fn write_config(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(name);
        fs::write(&path, contents).expect("write sticky config");
        path
    }

    fn init_context(config_path: Option<&Path>, plugin_dir: Option<&Path>) -> PluginInitContext {
        let mut entries = AbiVec::new();

        if let Some(path) = config_path {
            entries.push(KVPair {
                name: AbiString::from(CONFIG_ENV),
                value: AbiString::from(path.to_string_lossy().as_ref()),
            });
        }

        PluginInitContext {
            library_path: AbiString::from("/tmp/libexample-l4-sticky.so"),
            plugin_dir: plugin_dir
                .map(|path| AbiOption::Some(AbiString::from(path.to_string_lossy().as_ref())))
                .unwrap_or_else(AbiOption::None),
            host_version: SemVer {
                major: 1,
                minor: 0,
                patch: 0,
            },
            env: EnvSnapshot { entries },
        }
    }

    // fn route_context(listener: &str, peer: &str, protocol: Protocol) ->
    // HttpRouteContext {     HttpRouteContext {
    //         listener: AbiString::from(listener),
    //         peer: AbiString::from(peer),
    //         route_hint: AbiOption::None(),
    //         protocol,
    //     }
    // }

    #[test]
    fn manifest_exports_tcp_and_udp() {
        let library = get_library();

        assert_eq!(library.manifest.name.as_str(), "example-l4-sticky");
        assert!(
            library
                .create_tcp
                .match_owned(|create| create().is_ok(), || false)
        );
        assert!(
            library
                .create_udp
                .match_owned(|create| create().is_ok(), || false)
        );
    }

    #[test]
    fn parses_config_and_selects_shared_backend_for_tcp_and_udp() {
        let config = match LoadedConfig::try_from(FileConfig {
            ttl_secs: default_ttl_secs(),
            fallback_backends: vec!["127.0.0.1:9001".to_string(), "127.0.0.1:9002".to_string()],
            listeners: vec![super::ListenerConfig {
                name: "listener-a".to_string(),
                backends: vec!["127.0.0.1:9101".to_string(), "127.0.0.1:9102".to_string()],
            }],
        }) {
            Ok(c) => c,
            Err(e) => panic!("config parse error: {}", plugin_error_message(&e)),
        };

        let ip = parse_client_ip("192.0.2.10:54321").expect("client ip");
        let tcp_backend = choose_backend("listener-a", ip, config.pool_for_listener("listener-a"));
        let udp_backend = choose_backend("listener-a", ip, config.pool_for_listener("listener-a"));

        assert_eq!(tcp_backend, udp_backend);
        assert!(
            config
                .pool_for_listener("listener-a")
                .contains(&tcp_backend)
        );
    }

    #[test]
    fn validate_backend_list_normalization() {
        let backends = match normalize_backends(&[" 127.0.0.1:9001 ".to_string()], "test backends")
        {
            Ok(b) => b,
            Err(e) => panic!("normalize failed: {}", plugin_error_message(&e)),
        };
        assert_eq!(backends, vec!["127.0.0.1:9001".to_string()]);
    }

    #[test]
    fn config_rejects_empty_backend_pool() {
        match LoadedConfig::try_from(FileConfig {
            ttl_secs: default_ttl_secs(),
            fallback_backends: vec![],
            listeners: vec![],
        }) {
            Ok(_) => panic!("expected error for empty config"),
            Err(e) => {
                assert!(plugin_error_message(&e).contains("must define at least one listener pool"))
            }
        }
    }

    #[test]
    fn route_context_uses_client_ip_not_port() {
        let ip1 = parse_client_ip("192.0.2.10:10000").expect("ip1");
        let ip2 = parse_client_ip("192.0.2.10:20000").expect("ip2");
        let config = match LoadedConfig::try_from(FileConfig {
            ttl_secs: default_ttl_secs(),
            fallback_backends: vec!["127.0.0.1:9001".to_string(), "127.0.0.1:9002".to_string()],
            listeners: vec![],
        }) {
            Ok(c) => c,
            Err(e) => panic!("config parse error: {}", plugin_error_message(&e)),
        };

        let pool = config.pool_for_listener("missing-listener");
        let backend1 = choose_backend("listener-a", ip1, pool);
        let backend2 = choose_backend("listener-a", ip2, pool);

        assert_eq!(backend1, backend2);
    }

    #[test]
    fn resolve_config_path_prefers_env_then_plugin_dir_then_default() {
        let tempdir = TempDir::new().expect("tempdir");
        let env_config = tempdir.path().join("env.toml");
        let plugin_dir = tempdir.path().join("plugin");
        fs::create_dir(&plugin_dir).expect("plugin dir");

        let env_ctx = init_context(Some(&env_config), Some(&plugin_dir));
        assert_eq!(resolve_config_path(&env_ctx), env_config);

        let plugin_ctx = init_context(None, Some(&plugin_dir));
        assert_eq!(
            resolve_config_path(&plugin_ctx),
            plugin_dir.join("sticky-l4.toml")
        );

        let default_ctx = init_context(None, None);
        assert_eq!(
            resolve_config_path(&default_ctx),
            PathBuf::from("sticky-l4.toml")
        );
    }

    #[test]
    fn load_state_and_entrypoints_cover_route_and_transport_hooks() {
        let tempdir = TempDir::new().expect("tempdir");
        let config_path = write_config(
            &tempdir,
            "sticky-l4.toml",
            r#"
ttl_secs = 30
fallback_backends = ["127.0.0.1:9001", "127.0.0.1:9002"]

[[listeners]]
name = "listener-a"
backends = ["127.0.0.1:9101", "127.0.0.1:9102"]
"#,
        );
        let ctx = init_context(Some(&config_path), None);

        let state = match load_state(&ctx) {
            Ok(state) => state,
            Err(err) => panic!("load_state failed: {}", plugin_error_message(&err)),
        };

        let ip = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        let backend = match state.select_backend("listener-a", ip) {
            Ok(backend) => backend,
            Err(err) => panic!("select_backend failed: {}", plugin_error_message(&err)),
        };
        assert!(matches!(
            backend.as_str(),
            "127.0.0.1:9101" | "127.0.0.1:9102"
        ));

        let cached_backend = match state.select_backend("listener-a", ip) {
            Ok(backend) => backend,
            Err(err) => panic!(
                "cached select_backend failed: {}",
                plugin_error_message(&err)
            ),
        };
        assert_eq!(backend, cached_backend);

        let shared = Arc::new(Mutex::new(SharedInitState::default()));
        assert!((sticky_init)(std::ptr::null_mut(), &ctx as *const _).is_err());

        let tcp_module = create_tcp().match_owned(|module| module, |_| panic!("tcp module"));
        let udp_module = create_udp().match_owned(|module| module, |_| panic!("udp module"));

        assert!((tcp_module.init)(tcp_module.instance, &ctx as *const _).is_ok());
        assert!((udp_module.init)(udp_module.instance, &ctx as *const _).is_ok());

        // let mut route_ctx = route_context("listener-a", "192.0.2.10:12345",
        // Protocol::Tcp); let route = sticky_on_route(tcp_module.instance, &mut
        // route_ctx as *mut _).match_owned(     |route| route,
        //     |err| panic!("route error: {}", plugin_error_message(&err)),
        // );
        // assert_eq!(route.action as u8, RouteAction::ReplaceUpstream as u8);
        // assert_eq!(
        //     route
        //         .upstream_to_set
        //         .as_ref()
        //         .map(|upstream| (upstream.host.as_str(), upstream.port)),
        //     Some((
        //         backend.rsplit_once(':').expect("backend").0,
        //         backend
        //             .rsplit_once(':')
        //             .expect("backend")
        //             .1
        //             .parse::<u16>()
        //             .expect("port")
        //     ))
        // );

        // let mut invalid_route_ctx = route_context("listener-a", "not-a-socket",
        // Protocol::Udp); let invalid_route =
        // sticky_on_route(udp_module.instance, &mut invalid_route_ctx as *mut _)
        //     .match_owned(
        //         |route| route,
        //         |err| panic!("route error: {}", plugin_error_message(&err)),
        //     );
        // assert_eq!(invalid_route.action as u8, RouteAction::Keep as u8);

        assert_eq!(
            (tcp_module.on_connect.clone().unwrap())(tcp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );
        assert_eq!(
            (tcp_module.on_data.clone().unwrap())(tcp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );
        assert_eq!(
            (tcp_module.on_half_close.clone().unwrap())(tcp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );
        assert_eq!(
            (tcp_module.on_close.clone().unwrap())(tcp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );
        assert_eq!(
            (udp_module.on_datagram.clone().unwrap())(udp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );
        assert_eq!(
            (udp_module.on_session_start.clone().unwrap())(udp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );
        assert_eq!(
            (udp_module.on_session_end.clone().unwrap())(udp_module.instance, ptr::null_mut())
                .match_owned(
                    |outcome| outcome.action as u8,
                    |err| panic!("hook error: {}", plugin_error_message(&err))
                ),
            HookAction::Continue as u8
        );

        sticky_shutdown(ptr::null_mut());
        sticky_shutdown(tcp_module.instance);
        sticky_shutdown(udp_module.instance);

        let _ = shared;
    }

    #[test]
    fn ensure_state_caches_init_failures() {
        let tempdir = TempDir::new().expect("tempdir");
        let missing = tempdir.path().join("missing.toml");
        let ctx = init_context(Some(&missing), None);
        let slot = Arc::new(Mutex::new(SharedInitState::default()));

        let first = ensure_state(&slot, &ctx).expect_err("expected missing config error");
        let second = ensure_state(&slot, &ctx).expect_err("expected cached missing config error");

        assert!(plugin_error_message(&first).contains("failed to read sticky config"));
        assert_eq!(plugin_error_message(&first), plugin_error_message(&second));
    }
}
