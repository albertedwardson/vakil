//! Subprocess e2e for `vakil-cli` startup.
//!
//! This test boots the real CLI binary, loads a real plugin cdylib, and drives
//! live TCP/UDP traffic through the runtime so `vakil-runtime` startup paths
//! are covered by an actual process boundary.

use std::env;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vakil_tests_common::{reserve_port, wait_for_port};

#[test]
fn vakil_cli_starts_loads_plugin_and_handles_l4_traffic() {
    let listen_port = reserve_port();
    let plugin_path = build_example_noop_plugin();
    let cli_binary = build_vakil_cli_binary();
    let config_path = write_runtime_config(listen_port, &plugin_path);

    let (mut child, stderr, stderr_reader) = spawn_cli(&cli_binary, &config_path);

    wait_for_buffer_contains(&stderr, &mut child, "[example-noop] init:");
    wait_for_port(listen_port);

    send_udp_payload(listen_port, b"ping");
    wait_for_buffer_contains(
        &stderr,
        &mut child,
        "[example-noop] udp: peer=127.0.0.1 payload_bytes=4",
    );

    assert!(child.try_wait().expect("poll vakil-cli").is_none());

    let _ = child.kill();
    let _ = child.wait();
    let stderr_output = stderr_reader.join().expect("read vakil-cli stderr");

    assert!(stderr_output.contains("[example-noop] init:"));
    assert!(stderr_output.contains("[example-noop] udp: peer=127.0.0.1 payload_bytes=4"));
}

fn build_example_noop_plugin() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "-p", "example-noop"])
        .status()
        .expect("cargo build example-noop");
    assert!(status.success(), "example-noop build failed");

    locate_dynamic_library("example_noop")
}

fn build_vakil_cli_binary() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "-p", "vakil-cli"])
        .status()
        .expect("cargo build vakil-cli");
    assert!(status.success(), "vakil-cli build failed");

    let binary = workspace_target_debug_dir().join(if cfg!(windows) {
        "vakil-cli.exe"
    } else {
        "vakil-cli"
    });

    assert!(
        binary.exists(),
        "vakil-cli binary missing at {}",
        binary.display()
    );
    binary
}

fn spawn_cli(
    binary: &Path,
    config_path: &Path,
) -> (Child, Arc<Mutex<String>>, thread::JoinHandle<String>) {
    let mut command = Command::new(binary);
    command
        .arg("--config")
        .arg(config_path)
        .env("RUST_LOG", "info")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());

    let mut child = command.spawn().expect("spawn vakil-cli");
    let stderr = child.stderr.take().expect("capture stderr");
    let output = Arc::new(Mutex::new(String::new()));
    let reader_output = Arc::clone(&output);

    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();

        loop {
            let bytes_read = reader.read_line(&mut line).expect("read vakil-cli stderr");
            if bytes_read == 0 {
                break;
            }

            reader_output
                .lock()
                .expect("lock stderr buffer")
                .push_str(&line);
            line.clear();
        }

        reader_output.lock().expect("lock stderr buffer").clone()
    });

    (child, output, reader)
}

fn wait_for_buffer_contains(output: &Arc<Mutex<String>>, child: &mut Child, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(15);

    loop {
        if child.try_wait().expect("poll vakil-cli").is_some() {
            panic!("vakil-cli exited before logging {needle}");
        }

        if output.lock().expect("lock stderr buffer").contains(needle) {
            return;
        }

        if Instant::now() >= deadline {
            panic!("timed out waiting for log line: {needle}");
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn send_udp_payload(port: u16, payload: &[u8]) {
    let socket = UdpSocket::bind(("127.0.0.1", 0)).expect("bind udp client");
    socket
        .send_to(payload, ("127.0.0.1", port))
        .expect("send udp payload");
}

fn write_runtime_config(listen_port: u16, plugin_path: &Path) -> PathBuf {
    let config_path = std::env::temp_dir().join(format!(
        "vakil-cli-startup-{}-{}.toml",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos()
    ));

    let config = format!(
        "listen_addr = \"127.0.0.1:{listen_port}\"\nplugin_paths = \"{}\"\n",
        plugin_path.display()
    );

    fs::write(&config_path, config).expect("write vakil-cli config");
    config_path
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
    if let Ok(target_dir) = env::var("CARGO_TARGET_DIR") {
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
