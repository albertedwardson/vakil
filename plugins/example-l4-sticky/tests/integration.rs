// use stabby::option::Option as AbiOption;
// use stabby::string::String as AbiString;
// use std::env;
// use std::io::{Read, Write};
// use std::net::{TcpListener, TcpStream, UdpSocket};
// use std::process::Command;
// use std::sync::mpsc;
// use std::thread;
// use std::time::{Duration, Instant};
// use vakil_plugin_host::LoadedPlugin;

// // #[test]
// fn integration_loads_cdylib_and_routes() {
//     // Start ephemeral TCP and UDP echo backends
//     let tcp_listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind
// tcp");     let port = tcp_listener.local_addr().expect("local addr").port();

//     let (tcp_ready_tx, tcp_ready_rx) = mpsc::channel();
//     let tcp_handle = thread::spawn(move || {
//         tcp_ready_tx.send(()).unwrap();
//         // accept single connection, echo data, then exit
//         if let Ok((mut stream, _)) = tcp_listener.accept() {
//             let mut buf = [0u8; 1024];
//             if let Ok(n) = stream.read(&mut buf) {
//                 let _ = stream.write_all(&buf[..n]);
//             }
//         }
//     });

//     let udp_socket = UdpSocket::bind(("127.0.0.1", port)).expect("bind udp");
//     udp_socket
//         .set_read_timeout(Some(Duration::from_secs(5)))
//         .unwrap();
//     let (udp_ready_tx, udp_ready_rx) = mpsc::channel();
//     let udp_handle = thread::spawn(move || {
//         udp_ready_tx.send(()).unwrap();
//         let mut buf = [0u8; 1500];
//         let deadline = Instant::now() + Duration::from_secs(30);

//         loop {
//             match udp_socket.recv_from(&mut buf) {
//                 Ok((n, src)) => {
//                     let _ = udp_socket.send_to(&buf[..n], src);
//                     break;
//                 }
//                 Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
//                     if Instant::now() >= deadline {
//                         panic!("timed out waiting for udp datagram");
//                     }
//                 }
//                 Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {
//                     if Instant::now() >= deadline {
//                         panic!("timed out waiting for udp datagram");
//                     }
//                 }
//                 Err(err) => panic!("recv udp: {}", err),
//             }
//         }
//     });

//     // wait for servers to be ready
//     tcp_ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
//     udp_ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();

//     // Build plugin cdylib
//     let status = Command::new("cargo")
//         .args(["build", "-p", "example-l4-sticky"])
//         .status()
//         .expect("cargo build failed to start");
//     assert!(status.success());

//     // Locate built library
//     let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_|
// ".".to_string());     let target =
// std::path::Path::new(&manifest_dir).join("../../target/debug");     let mut
// found = None;     for entry in std::fs::read_dir(&target).expect("read
// target/debug") {         let entry = entry.expect("entry");
//         let path = entry.path();
//         if path
//             .file_name()
//             .and_then(|n| n.to_str())
//             .map(|s| s.contains("example_l4_sticky"))
//             .unwrap_or(false)
//         {
//             found = Some(path);
//             break;
//         }
//     }

//     let libpath = found.expect("built cdylib not found");

//     // Write sticky config pointing to ephemeral backend port
//     let lib_dir = libpath.parent().expect("lib parent");
//     let cfg_path = lib_dir.join("sticky-l4.toml");
//     let cfg = format!(
//         r#"ttl_secs = 30
// fallback_backends = ["127.0.0.1:{}"]
// "#,
//         port
//     );
//     std::fs::write(&cfg_path, cfg).expect("write cfg");

//     // Load plugin via host loader
//     let plugin = LoadedPlugin::load(libpath).expect("load plugin");
//     assert_eq!(plugin.name(), "example-l4-sticky");
//     assert!(plugin.has_tcp());
//     assert!(plugin.has_udp());

//     // Verify tcp module route points to backend and backend echoes
//     let tcp_module = plugin.modules.tcp.as_ref().expect("tcp module");
//     // let mut tcp_route = HttpRouteContext {
//     //     listener: AbiString::from("listener-a"),
//     //     peer: AbiString::from("127.0.0.1:12345"),
//     //     route_hint: AbiOption::None(),
//     //     protocol: Protocol::Tcp,
//     // };

//     let tcp_decision =
//         (tcp_module.on_route.clone().unwrap())(tcp_module.instance, &mut
// tcp_route as *mut _)             .match_owned(
//                 |r| r,
//                 |e| {
//                     panic!(
//                         "route error: {}",
//                         e.message.as_ref().map(|m| m.as_str()).unwrap_or("")
//                     )
//                 },
//             );

//     // assert!(tcp_decision.upstream_to_set.is_some());
//     // let upstream = tcp_decision.upstream_to_set.as_ref().unwrap();
//     assert_eq!(upstream.host.as_str(), "127.0.0.1");
//     assert_eq!(upstream.port, port);

//     // Connect to backend and test echo
//     let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect
// backend");     stream.write_all(b"hello").expect("write");
//     let mut buf = [0u8; 5];
//     stream.read_exact(&mut buf).expect("read echo");
//     assert_eq!(&buf, b"hello");

//     // Verify udp module route similarly
//     let udp_module = plugin.modules.udp.as_ref().expect("udp module");
//     let mut udp_route = HttpRouteContext {
//         listener: AbiString::from("listener-a"),
//         peer: AbiString::from("127.0.0.1:12345"),
//         route_hint: AbiOption::None(),
//         protocol: Protocol::Udp,
//     };

//     let udp_decision =
//         (udp_module.on_route.clone().unwrap())(udp_module.instance, &mut
// udp_route as *mut _)             .match_owned(
//                 |r| r,
//                 |e| {
//                     panic!(
//                         "route error: {}",
//                         e.message.as_ref().map(|m| m.as_str()).unwrap_or("")
//                     )
//                 },
//             );

//     // assert!(udp_decision.upstream_to_set.is_some());
//     // let udp_up = udp_decision.upstream_to_set.as_ref().unwrap();
//     assert_eq!(udp_up.host.as_str(), "127.0.0.1");
//     assert_eq!(udp_up.port, port);

//     // Send UDP msg and expect echoed response
//     let client = UdpSocket::bind(("127.0.0.1", 0)).expect("bind udp client");
//     client
//         .set_read_timeout(Some(Duration::from_secs(2)))
//         .unwrap();
//     client
//         .send_to(b"ping", ("127.0.0.1", port))
//         .expect("send udp");
//     let mut rbuf = [0u8; 1500];
//     let (n, _src) = client.recv_from(&mut rbuf).expect("recv udp");
//     assert_eq!(&rbuf[..n], b"ping");

//     // join server threads
//     let _ = tcp_handle.join();
//     let _ = udp_handle.join();
// }
