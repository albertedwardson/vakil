//! Runtime orchestration for Vakil.
//!
//! This crate owns startup configuration, plugin loading, and server
//! bootstrapping.

use anyhow::{Context, Result, bail};
use log::{debug, error, info};
use pingora_core::server::Server;
use pingora_core::server::configuration::Opt;
use std::env;
use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use vakil_http::{ProxySettings, attach_proxy_service};
use vakil_l4::{run_tcp_server, run_udp_server};
use vakil_plugin_api::{PluginHttpModule, PluginTcpModule, PluginUdpModule};
use vakil_plugin_host::LoadedPlugin;

mod http;
mod l4;

/// Runtime configuration derived from environment and optional config file.
#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    /// Address where Pingora listens for downstream connections.
    pub http_listen_addr: String,
    pub tcp_listen_addr: String,
    pub udp_listen_addr: String,
    /// Plugin library files or directories to load at startup.
    pub plugin_paths: Vec<PathBuf>,
}

impl RuntimeConfig {
    /// Load runtime configuration from environment variables.
    ///
    /// Supported variables:
    /// - `VAKIL_CONFIG` for an optional strict key/value config file
    /// - `VAKIL_HTTP_LISTEN_ADDR`
    /// - `VAKIL_TCP_LISTEN_ADDR`
    /// - `VAKIL_UDP_LISTEN_ADDR`
    /// - `VAKIL_PLUGIN_PATHS`
    /// - `VAKIL_PATH_SEP`
    pub fn from_env() -> Result<Self> {
        let mut config = Self {
            http_listen_addr: env::var("VAKIL_HTTP_LISTEN_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:12345".to_string()),
            tcp_listen_addr: env::var("VAKIL_TCP_LISTEN_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:12346".to_string()),
            udp_listen_addr: env::var("VAKIL_UDP_LISTEN_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:12347".to_string()),
            plugin_paths: parse_plugin_paths(
                &env::var("VAKIL_PLUGIN_PATHS").unwrap_or_default(),
                &env::var("VAKIL_PATH_SEP").unwrap_or_else(|_| ";".to_string()),
            ),
        };

        if let Ok(config_path) = env::var("VAKIL_CONFIG") {
            let overrides = parse_config_file(Path::new(&config_path))?;
            if let Some(http_listen_addr) = overrides.http_listen_addr {
                config.http_listen_addr = http_listen_addr;
            }
            if let Some(plugin_paths) = overrides.plugin_paths {
                config.plugin_paths = plugin_paths;
            }
        }

        Ok(config)
    }
}

/// Fully built runtime instance.
#[derive(Clone, Debug)]
pub struct Runtime {
    config: RuntimeConfig,
    plugins: Arc<Vec<LoadedPlugin>>,
}

/// Adapter that invokes plugin modules
#[derive(Clone, Debug)]
struct PluginProxyHooks {
    http_modules: Arc<Vec<PluginHttpModule>>,
    tcp_modules: Arc<Vec<PluginTcpModule>>,
    udp_modules: Arc<Vec<PluginUdpModule>>,
}

// Safety: PluginProxyHooks contains raw pointers to plugin instances. The
// runtime ensures these pointers remain valid for the lifetime of the hooks
// object and serializes access through the hook invocation model. Marking
// this type Send/Sync is required so it can be shared with the proxy service
// background threads.
unsafe impl Send for PluginProxyHooks {}
unsafe impl Sync for PluginProxyHooks {}

impl PluginProxyHooks {
    fn from_loaded(plugins: Arc<Vec<LoadedPlugin>>) -> Self {
        let mut http_modules = Vec::new();
        let mut tcp_modules = Vec::new();
        let mut udp_modules = Vec::new();

        for p in plugins.iter() {
            if let Some(http) = p.modules.http.as_ref() {
                http_modules.push(http.clone());
            }

            if let Some(tcp) = p.modules.tcp.as_ref() {
                tcp_modules.push(tcp.clone());
            }

            if let Some(udp) = p.modules.udp.as_ref() {
                udp_modules.push(udp.clone());
            }
        }

        // sort by priority ascending
        http_modules.sort_by_key(|m| m.priority);
        tcp_modules.sort_by_key(|m| m.priority);
        udp_modules.sort_by_key(|m| m.priority);

        Self {
            http_modules: http_modules.into(),
            tcp_modules: tcp_modules.into(),
            udp_modules: udp_modules.into(),
        }
    }
}

impl Runtime {
    /// Build a runtime by loading all configured plugins at startup.
    pub fn build(config: RuntimeConfig) -> Result<Self> {
        info!(
            "building runtime for {} configured plugin path(s)",
            config.plugin_paths.len()
        );
        let plugins = load_plugins(&config.plugin_paths)?;
        info!("runtime loaded {} plugin(s)", plugins.len());
        debug!("{:?}", config);

        Ok(Self {
            config,
            plugins: plugins.into(),
        })
    }

    /// Start the Pingora server with the configured proxy service.
    pub async fn run(self) -> Result<()> {
        info!("starting http runtime on {}", self.config.http_listen_addr);
        debug!("{:?}", self);
        let mut server = Server::new(Some(Opt::default())).context("create pingora server")?;
        server.bootstrap();

        let plugin_hooks = Arc::new(PluginProxyHooks::from_loaded(self.plugins.clone()));
        // ---------------------------------------------------------------------
        // HTTP service (Pingora)
        // ---------------------------------------------------------------------
        let http_settings = ProxySettings::new(self.config.http_listen_addr.clone());
        let http_hooks = plugin_hooks.clone();
        attach_proxy_service(&mut server, http_settings.clone(), (*http_hooks).clone());

        // ---------------------------------------------------------------------
        // TCP service (Rama)
        // ---------------------------------------------------------------------
        let mut nursery = tokio::task::JoinSet::new();
        if let Ok(tcp_addr) = self.config.tcp_listen_addr.parse::<SocketAddr>() {
            let tcp_hooks = plugin_hooks.clone();
            nursery.spawn(async move {
                info!("starting tcp on {}", self.config.tcp_listen_addr);
                if let Err(e) = run_tcp_server(tcp_addr, (*tcp_hooks).clone()).await {
                    log::error!("TCP server failed: {}", e);
                }
            });
        } else {
            log::warn!(
                "invalid TCP listen address: {}",
                self.config.tcp_listen_addr
            );
        }

        // ---------------------------------------------------------------------
        // UDP service (Rama)
        // ---------------------------------------------------------------------
        if let Ok(udp_addr) = self.config.udp_listen_addr.parse::<SocketAddr>() {
            let udp_hooks = plugin_hooks.clone();
            nursery.spawn(async move {
                info!("starting udp on {}", self.config.udp_listen_addr);
                if let Err(e) = run_udp_server(udp_addr, (*udp_hooks).clone()).await {
                    log::error!("UDP server failed: {}", e);
                }
            });
        } else {
            log::warn!(
                "invalid UDP listen address: {}",
                self.config.udp_listen_addr
            );
        }

        info!(
            "runtime attached proxy services for {} loaded plugin(s)",
            self.plugins.clone().len()
        );
        nursery.spawn_blocking(move || {
            server.run_forever();
        });
        while let Some(res) = nursery.join_next().await {
            if let Err(err) = res {
                error!("one of the servers completed with error: {:?}", err);
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
struct RuntimeOverrides {
    http_listen_addr: Option<String>,
    plugin_paths: Option<Vec<PathBuf>>,
}

fn parse_config_file(path: &Path) -> Result<RuntimeOverrides> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("read runtime config file {}", path.display()))?;

    let mut overrides = RuntimeOverrides {
        http_listen_addr: None,
        plugin_paths: None,
    };

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            bail!("invalid config line: {}", line);
        };

        let key = key.trim();
        let value = parse_config_value(value.trim())?;

        match key {
            "http_listen_addr" => {
                if overrides.http_listen_addr.is_some() {
                    bail!(
                        "duplicate http_listen_addr in config file: {}",
                        path.display()
                    );
                }

                value.parse::<SocketAddr>().with_context(|| {
                    format!("invalid http_listen_addr value in {}", path.display())
                })?;
                overrides.http_listen_addr = Some(value);
            }
            "plugin_paths" => {
                if overrides.plugin_paths.is_some() {
                    bail!("duplicate plugin_paths in config file: {}", path.display());
                }

                overrides.plugin_paths = Some(parse_plugin_paths_strict(&value)?);
            }
            _ => bail!("unknown config key {} in {}", key, path.display()),
        }
    }

    Ok(overrides)
}

fn parse_plugin_paths(raw: &str, separator: &str) -> Vec<PathBuf> {
    if raw.trim().is_empty() {
        return Vec::new();
    }

    raw.split(separator)
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(PathBuf::from)
        .collect()
}

fn parse_plugin_paths_strict(raw: &str) -> Result<Vec<PathBuf>> {
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }

    let mut paths = Vec::new();

    for (index, entry) in raw.split(';').enumerate() {
        let entry = entry.trim();
        if entry.is_empty() {
            bail!("invalid empty plugin path entry at position {}", index + 1);
        }

        paths.push(PathBuf::from(entry));
    }

    Ok(paths)
}

fn parse_config_value(input: &str) -> Result<String> {
    let Some(stripped) = input
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    else {
        bail!("config values must be quoted strings: {}", input);
    };

    Ok(stripped.to_string())
}

fn load_plugins(paths: &[PathBuf]) -> Result<Vec<LoadedPlugin>> {
    let mut plugins = Vec::new();

    for path in paths {
        if path.is_dir() {
            debug!("scanning plugin directory {}", path.display());
            for entry in fs::read_dir(path)
                .with_context(|| format!("read plugin directory {}", path.display()))?
            {
                let entry = entry?;
                let candidate = entry.path();
                if candidate.is_file() && is_plugin_library(&candidate) {
                    debug!("loading plugin library {}", candidate.display());
                    plugins.push(LoadedPlugin::load(&candidate)?);
                }
            }
        } else if path.is_file() {
            info!("loading plugin library {}", path.display());
            plugins.push(LoadedPlugin::load(path)?);
        } else {
            debug!("skipping non-existent plugin path {}", path.display());
        }
    }

    Ok(plugins)
}

fn is_plugin_library(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext == env::consts::DLL_EXTENSION)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn parse_plugin_paths_splits_and_trims_entries() {
        let paths = parse_plugin_paths("  /tmp/one ; ; /tmp/two  ", ";");

        assert_eq!(
            paths,
            vec![PathBuf::from("/tmp/one"), PathBuf::from("/tmp/two")]
        );
    }

    #[test]
    fn parse_config_file_reads_listen_addr_and_plugins() {
        let file_name = format!(
            "vakil-runtime-test-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(file_name);

        fs::write(
            &path,
            r#"
http_listen_addr = "127.0.0.1:9090"
plugin_paths = "./one; ./two"
"#,
        )
        .expect("write runtime config");

        let config = parse_config_file(&path).expect("parse runtime config");

        assert_eq!(config.http_listen_addr.as_deref(), Some("127.0.0.1:9090"));
        assert_eq!(
            config.plugin_paths.as_ref().map(|paths| paths.len()),
            Some(2)
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn parse_config_file_rejects_unknown_keys() {
        let file_name = format!(
            "vakil-runtime-invalid-key-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(file_name);

        fs::write(
            &path,
            r#"
http_listen_addr = "127.0.0.1:9090"
bogus = "nope"
"#,
        )
        .expect("write runtime config");

        let err = parse_config_file(&path).expect_err("reject unknown key");
        assert!(err.to_string().contains("unknown config key bogus"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn parse_config_file_rejects_duplicate_keys() {
        let file_name = format!(
            "vakil-runtime-duplicate-key-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(file_name);

        fs::write(
            &path,
            r#"
http_listen_addr = "127.0.0.1:9090"
http_listen_addr = "127.0.0.1:9091"
"#,
        )
        .expect("write runtime config");

        let err = parse_config_file(&path).expect_err("reject duplicate key");
        assert!(err.to_string().contains("duplicate http_listen_addr"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn parse_config_file_rejects_invalid_listen_addr() {
        let file_name = format!(
            "vakil-runtime-invalid-addr-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(file_name);

        fs::write(
            &path,
            r#"
http_listen_addr = "not-an-addr"
"#,
        )
        .expect("write runtime config");

        let err = parse_config_file(&path).expect_err("reject invalid listen addr");
        assert!(err.to_string().contains("invalid http_listen_addr value"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn parse_config_file_rejects_empty_plugin_path_segments() {
        let file_name = format!(
            "vakil-runtime-invalid-plugin-paths-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(file_name);

        fs::write(
            &path,
            r#"
plugin_paths = "./one;;./two"
"#,
        )
        .expect("write runtime config");

        let err = parse_config_file(&path).expect_err("reject malformed plugin paths");
        assert!(err.to_string().contains("invalid empty plugin path entry"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn build_accepts_empty_plugin_list() {
        let runtime = Runtime::build(RuntimeConfig {
            http_listen_addr: "127.0.0.1:0".to_string(),
            tcp_listen_addr: "127.0.0.1:1".to_string(),
            udp_listen_addr: "127.0.0.1:2".to_string(),
            plugin_paths: Vec::new(),
        })
        .expect("build runtime without plugins");

        let _ = runtime;
    }

    #[test]
    fn from_env_prefers_config_file_over_env_defaults() {
        let _guard = ENV_LOCK.lock().expect("lock env test");

        let file_name = format!(
            "vakil-runtime-env-test-{}-{}.toml",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock before unix epoch")
                .as_nanos()
        );
        let path = std::env::temp_dir().join(file_name);

        fs::write(
            &path,
            r#"
http_listen_addr = "127.0.0.1:9091"
plugin_paths = "./alpha;./beta"
"#,
        )
        .expect("write runtime config");

        unsafe {
            std::env::set_var("VAKIL_CONFIG", &path);
            std::env::set_var("VAKIL_LISTEN_ADDR", "127.0.0.1:9999");
            std::env::set_var("VAKIL_PLUGIN_PATHS", "./ignored");
            std::env::set_var("VAKIL_PATH_SEP", ";");
        }

        let config = RuntimeConfig::from_env().expect("load runtime config from env");

        assert_eq!(config.http_listen_addr, "127.0.0.1:9091");
        assert_eq!(
            config.plugin_paths,
            vec![PathBuf::from("./alpha"), PathBuf::from("./beta")]
        );

        unsafe {
            std::env::remove_var("VAKIL_CONFIG");
            std::env::remove_var("VAKIL_LISTEN_ADDR");
            std::env::remove_var("VAKIL_PLUGIN_PATHS");
            std::env::remove_var("VAKIL_PATH_SEP");
        }

        let _ = fs::remove_file(&path);
    }
}
