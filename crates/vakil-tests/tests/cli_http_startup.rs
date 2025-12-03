// NOTE: This integration test has been observed to be flaky on CI runners.
// On GitHub Actions we intermittently see a `Connection reset by peer` when
// reading the proxied HTTP response. Locally we've hardened the test helper
// to tolerate EOF/ConnectionReset, but the root cause on CI is still unclear
// (possible timing/race in startup or subtle socket teardown behavior).
//
// TODO: Investigate CI-only failure mode: capture full logs, run with
// `RUST_BACKTRACE=full`, and consider instrumenting the proxy close path.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use vakil_tests_common::{reserve_port, wait_for_port};

#[test]
fn vakil_cli_starts_and_runs_http_plugin() {
    let plugin_path = build_example_http_plugin();
    let cli_binary = build_vakil_cli_binary();
    run_http_plugin_scenario(&cli_binary, &plugin_path);
}

#[test]
#[ignore = "stress test; run with -- --ignored and optionally set VAKIL_STRESS_ITERS"]
fn vakil_cli_http_startup_stress_restarts() {
    let plugin_path = build_example_http_plugin();
    let cli_binary = build_vakil_cli_binary();
    let iterations = std::env::var("VAKIL_STRESS_ITERS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(25);

    for iteration in 0..iterations {
        eprintln!(
            "[cli_http_startup_stress] iteration {}/{}",
            iteration + 1,
            iterations
        );
        run_http_plugin_scenario(&cli_binary, &plugin_path);
    }
}

fn run_http_plugin_scenario(cli_binary: &Path, plugin_path: &Path) {
    let listen_port = reserve_port();
    let backend_port = reserve_port();
    let backend_hits = Arc::new(AtomicUsize::new(0));
    let backend_addr = format!("127.0.0.1:{}", backend_port);
    let (backend_ready_tx, backend_ready_rx) = mpsc::channel();

    let backend_thread = spawn_http_backend(backend_port, backend_hits.clone(), backend_ready_tx);
    let config_path = write_runtime_config(listen_port, plugin_path);

    let (mut child, stderr, stderr_reader) =
        spawn_cli(cli_binary, &config_path, Some(&backend_addr));

    backend_ready_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("backend ready");

    wait_for_buffer_contains(&stderr, &mut child, "[example-http-mw] init:");
    wait_for_port(listen_port);

    let upstream_response = wait_for_http_response_contains(
        &stderr,
        &mut child,
        listen_port,
        "POST",
        "/live",
        "hello runtime",
        "hello from backend",
    );
    assert!(
        upstream_response.contains("hello from backend"),
        "upstream_response={upstream_response:?}"
    );

    let local_reply = send_http_request(listen_port, "GET", "/local-reply", "");
    assert!(local_reply.contains("hello from example-http-mw"));

    wait_for_buffer_contains(&stderr, &mut child, "[example-http-mw] request:");
    wait_for_buffer_contains(&stderr, &mut child, "[example-http-mw] request-body:");
    wait_for_buffer_contains(&stderr, &mut child, "[example-http-mw] response:");
    wait_for_buffer_contains(&stderr, &mut child, "[example-http-mw] response-body:");
    wait_for_buffer_contains(&stderr, &mut child, "[example-http-mw] local-reply:");

    assert_eq!(backend_hits.load(Ordering::SeqCst), 1);

    backend_thread
        .join()
        .expect("join backend thread without panic");

    let _ = child.kill();
    let _ = child.wait();
    let stderr_output = stderr_reader.join().expect("read vakil-cli stderr");

    assert!(stderr_output.contains("[example-http-mw] init:"));
    assert!(stderr_output.contains("[example-http-mw] request:"));
    assert!(stderr_output.contains("[example-http-mw] response:"));

    let _ = fs::remove_file(&config_path);
}

fn build_example_http_plugin() -> PathBuf {
    let status = Command::new("cargo")
        .args(["build", "-p", "example-http-mw"])
        .status()
        .expect("cargo build example-http-mw");
    assert!(status.success(), "example-http-mw build failed");

    locate_dynamic_library("example_http_mw")
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
    backend_addr: Option<&str>,
) -> (Child, Arc<Mutex<String>>, thread::JoinHandle<String>) {
    let mut command = Command::new(binary);
    command
        .arg("--config")
        .arg(config_path)
        .env("RUST_LOG", "info")
        .envs(
            backend_addr
                .into_iter()
                .map(|value| ("VAKIL_HTTP_MW_BACKEND", value)),
        )
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

fn send_http_request(port: u16, method: &str, path: &str, body: &str) -> String {
    let mut stream =
        std::net::TcpStream::connect(("127.0.0.1", port)).expect("connect vakil-cli http");
    let request = if body.is_empty() {
        format!("{method} {path} HTTP/1.1\r\nHost: example.test\r\nConnection: close\r\n\r\n")
    } else {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: example.test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };

    stream
        .write_all(request.as_bytes())
        .expect("write http request");

    let mut response = Vec::new();
    let mut buffer = [0_u8; 1024];

    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(bytes_read) => response.extend_from_slice(&buffer[..bytes_read]),
            Err(err)
                if err.kind() == std::io::ErrorKind::ConnectionReset
                    || err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(err) => panic!("read http response: {err}"),
        }
    }

    String::from_utf8(response).expect("decode http response")
}

fn wait_for_http_response_contains(
    stderr: &Arc<Mutex<String>>,
    child: &mut Child,
    port: u16,
    method: &str,
    path: &str,
    body: &str,
    needle: &str,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);

    loop {
        if child.try_wait().expect("poll vakil-cli").is_some() {
            panic!(
                "vakil-cli exited before returning an HTTP response containing {needle:?}; stderr={:?}",
                stderr.lock().expect("lock stderr buffer").as_str()
            );
        }

        let response = send_http_request(port, method, path, body);
        if response.contains(needle) {
            return response;
        }

        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for HTTP response containing {needle:?}; last_response={response:?}; stderr={:?}",
                stderr.lock().expect("lock stderr buffer").as_str()
            );
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn spawn_http_backend(
    port: u16,
    hits: Arc<AtomicUsize>,
    ready_tx: mpsc::Sender<()>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind backend");
        ready_tx.send(()).expect("signal backend ready");
        let deadline = Instant::now() + Duration::from_secs(12);

        while Instant::now() < deadline {
            let (mut stream, _) = listener.accept().expect("accept backend request");
            let request = read_http_request(&mut stream);
            let request_text = String::from_utf8_lossy(&request);

            if !request_text.contains("POST /live HTTP/1.1")
                || !request_text.contains("hello runtime")
            {
                continue;
            }

            hits.fetch_add(1, Ordering::SeqCst);

            let response = b"HTTP/1.1 200 OK\r\nContent-Length: 18\r\nConnection: close\r\n\r\nhello from backend";
            stream.write_all(response).expect("write response");
            return;
        }

        panic!("backend timed out waiting for expected POST /live request");
    })
}

fn read_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    let mut content_length = None;

    loop {
        let bytes_read = stream.read(&mut buffer).expect("read backend request");
        if bytes_read == 0 {
            break;
        }

        request.extend_from_slice(&buffer[..bytes_read]);
        if content_length.is_none() && request.windows(4).any(|window| window == b"\r\n\r\n") {
            let request_text = String::from_utf8_lossy(&request);
            content_length = request_text.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                if name.eq_ignore_ascii_case("content-length") {
                    value.trim().parse::<usize>().ok()
                } else {
                    None
                }
            });
        }

        if let Some(expected_body_len) = content_length {
            if let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let body_len = request.len().saturating_sub(header_end + 4);
                if body_len >= expected_body_len {
                    break;
                }
            }
        } else if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }

    request
}

fn write_runtime_config(listen_port: u16, plugin_path: &Path) -> PathBuf {
    let config_path = std::env::temp_dir().join(format!(
        "vakil-cli-http-startup-{}-{}.toml",
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
