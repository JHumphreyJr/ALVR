//! Unix-socket IPC between the always-on server daemon (dashboard) and the OpenVR driver.

use alvr_packets::Haptics;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{Read, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
use alvr_common::{info, warn};

use crate::ViewsConfig;

pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("alvr")
}

pub fn daemon_marker_path() -> PathBuf {
    runtime_dir().join("server-daemon.json")
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("driver-ipc.sock")
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DaemonMarker {
    pub pid: u32,
    pub socket: String,
}

pub fn read_daemon_marker() -> Option<DaemonMarker> {
    let text = fs::read_to_string(daemon_marker_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn is_daemon_process_alive() -> bool {
    let Some(marker) = read_daemon_marker() else {
        return false;
    };
    PathBuf::from(&marker.socket).exists()
        && std::path::Path::new(&format!("/proc/{}", marker.pid)).exists()
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IpcFov {
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct IpcViewsConfig {
    pub ipd_m: f32,
    pub fov: [IpcFov; 2],
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum IpcServerEvent {
    ClientConnected,
    ClientDisconnected,
    Battery {
        device_id: u64,
        gauge_value: f32,
        is_plugged: bool,
    },
    PlayspaceSync {
        width: f32,
        height: f32,
    },
    ViewsConfig(IpcViewsConfig),
    RequestIdr,
    CaptureFrame,
    ShutdownSteamvr,
    RestartSteamvr,
}

#[derive(Serialize, Deserialize)]
pub enum IpcMessage {
    DriverHello,
    ServerEvent(IpcServerEvent),
    VideoConfig {
        buffer: Vec<u8>,
        codec: u8,
    },
    VideoNal {
        timestamp_ns: u64,
        buffer: Vec<u8>,
        is_idr: bool,
    },
    GetEncoderParams,
    EncoderParams {
        updated: bool,
        bitrate_bps: u64,
        framerate: f32,
    },
    Composed {
        timestamp_ns: u64,
        offset_ns: u64,
    },
    Present {
        timestamp_ns: u64,
        offset_ns: u64,
    },
    Haptics(Haptics),
}

pub(crate) fn write_message(stream: &mut UnixStream, message: &IpcMessage) -> bool {
    let Ok(payload) = bincode::serialize(message) else {
        return false;
    };
    let len = payload.len() as u32;
    stream.write_all(&len.to_le_bytes()).is_ok()
        && stream.write_all(&payload).is_ok()
        && stream.flush().is_ok()
}

fn read_message(stream: &mut UnixStream) -> Option<IpcMessage> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).ok()?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 16 * 1024 * 1024 {
        return None;
    }
    let mut payload = vec![0u8; len];
    stream.read_exact(&mut payload).ok()?;
    bincode::deserialize(&payload).ok()
}

pub struct DriverIpcServer {
    driver_connected: Arc<AtomicBool>,
    stream: Mutex<Option<UnixStream>>,
}

impl DriverIpcServer {
    pub fn start() -> Arc<Self> {
        let _ = fs::create_dir_all(runtime_dir());
        let sock = socket_path();
        let _ = fs::remove_file(&sock);

        let listener = UnixListener::bind(&sock).expect("bind driver-ipc.sock");
        listener.set_nonblocking(true).ok();

        let server = Arc::new(Self {
            driver_connected: Arc::new(AtomicBool::new(false)),
            stream: Mutex::new(None),
        });

        let marker = DaemonMarker {
            pid: std::process::id(),
            socket: sock.to_string_lossy().into_owned(),
        };
        if let Ok(text) = serde_json::to_string_pretty(&marker) {
            let _ = fs::write(daemon_marker_path(), text);
        }

        let server_clone = Arc::clone(&server);
        thread::Builder::new()
            .name("alvr-driver-ipc-server".into())
            .spawn(move || {
                loop {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            info!("OpenVR driver attached via IPC");
                            server_clone.driver_connected.store(true, Ordering::SeqCst);
                            *server_clone.stream.lock().unwrap() = Some(stream.try_clone().unwrap());

                            loop {
                                match read_message(&mut stream) {
                                    Some(IpcMessage::DriverHello) => {}
                                    Some(msg) => server_clone.handle_driver_message(msg),
                                    None => break,
                                }
                            }

                            warn!("OpenVR driver IPC disconnected");
                            server_clone.driver_connected.store(false, Ordering::SeqCst);
                            *server_clone.stream.lock().unwrap() = None;
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(50));
                        }
                        Err(e) => {
                            warn!("driver-ipc accept error: {e}");
                            thread::sleep(Duration::from_millis(200));
                        }
                    }
                }
            })
            .ok();

        server
    }

    fn handle_driver_message(&self, message: IpcMessage) {
        if let Some(handler) = DRIVER_MESSAGE_HANDLER.get() {
            handler(message);
        }
    }

    pub fn wait_for_driver(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.driver_connected.load(Ordering::SeqCst) {
                return true;
            }
            thread::sleep(Duration::from_millis(100));
        }
        false
    }

    pub fn is_driver_connected(&self) -> bool {
        self.driver_connected.load(Ordering::SeqCst)
    }

    pub fn send_event(&self, event: IpcServerEvent) {
        self.send_raw(IpcMessage::ServerEvent(event));
    }

    pub fn send_raw(&self, message: IpcMessage) {
        let mut guard = self.stream.lock().unwrap();
        if let Some(stream) = guard.as_mut() {
            if !write_message(stream, &message) {
                *guard = None;
                self.driver_connected.store(false, Ordering::SeqCst);
            }
        }
    }
}

static DRIVER_IPC_SERVER: std::sync::OnceLock<Arc<DriverIpcServer>> = std::sync::OnceLock::new();

pub fn start_driver_ipc_server() -> Arc<DriverIpcServer> {
    DRIVER_IPC_SERVER
        .get_or_init(DriverIpcServer::start)
        .clone()
}

pub fn driver_ipc_server() -> Option<Arc<DriverIpcServer>> {
    DRIVER_IPC_SERVER.get().cloned()
}

pub fn ensure_driver_ready(timeout: Duration) -> bool {
    if let Some(server) = driver_ipc_server() {
        if server.is_driver_connected() {
            return true;
        }
        info!("Waiting for OpenVR driver to attach ({:.0}s)…", timeout.as_secs_f32());
        server.wait_for_driver(timeout)
    } else {
        true
    }
}

static DRIVER_MESSAGE_HANDLER: std::sync::OnceLock<Box<dyn Fn(IpcMessage) + Send + Sync>> =
    std::sync::OnceLock::new();

pub fn set_driver_message_handler(handler: impl Fn(IpcMessage) + Send + Sync + 'static) {
    let _ = DRIVER_MESSAGE_HANDLER.set(Box::new(handler));
}

pub fn views_config_to_ipc(config: &ViewsConfig) -> IpcViewsConfig {
    IpcViewsConfig {
        ipd_m: config.local_view_transforms[1].position.x
            - config.local_view_transforms[0].position.x,
        fov: [
            IpcFov {
                left: config.fov[0].left,
                right: config.fov[0].right,
                up: config.fov[0].up,
                down: config.fov[0].down,
            },
            IpcFov {
                left: config.fov[1].left,
                right: config.fov[1].right,
                up: config.fov[1].up,
                down: config.fov[1].down,
            },
        ],
    }
}

pub struct DriverIpcClient {
    stream: Mutex<UnixStream>,
}

impl DriverIpcClient {
    pub fn connect() -> Option<Self> {
        let marker = read_daemon_marker()?;
        let mut stream = UnixStream::connect(&marker.socket).ok()?;
        if !write_message(&mut stream, &IpcMessage::DriverHello) {
            return None;
        }
        Some(Self {
            stream: Mutex::new(stream),
        })
    }

    fn send(&self, message: IpcMessage) -> bool {
        let mut stream = self.stream.lock().unwrap();
        write_message(&mut stream, &message)
    }

    pub fn run_event_reader(&self, mut dispatch: impl FnMut(IpcMessage) + Send + 'static) {
        let stream = self.stream.lock().unwrap().try_clone().unwrap();
        thread::Builder::new()
            .name("alvr-driver-ipc-client".into())
            .spawn(move || {
                let mut stream = stream;
                loop {
                    match read_message(&mut stream) {
                        Some(msg) => dispatch(msg),
                        None => break,
                    }
                }
            })
            .ok();
    }

    pub fn request_encoder_params(&self) {
        self.send(IpcMessage::GetEncoderParams);
    }

    pub fn send_video_config(&self, buffer: Vec<u8>, codec: u8) {
        self.send(IpcMessage::VideoConfig { buffer, codec });
    }

    pub fn send_video_nal(&self, timestamp_ns: u64, buffer: Vec<u8>, is_idr: bool) {
        self.send(IpcMessage::VideoNal {
            timestamp_ns,
            buffer,
            is_idr,
        });
    }

    pub fn send_composed(&self, timestamp_ns: u64, offset_ns: u64) {
        self.send(IpcMessage::Composed {
            timestamp_ns,
            offset_ns,
        });
    }

    pub fn send_present(&self, timestamp_ns: u64, offset_ns: u64) {
        self.send(IpcMessage::Present {
            timestamp_ns,
            offset_ns,
        });
    }

    pub fn send_haptics(&self, haptics: Haptics) {
        self.send(IpcMessage::Haptics(haptics));
    }
}

pub fn clear_daemon_marker() {
    let _ = fs::remove_file(daemon_marker_path());
    let _ = fs::remove_file(socket_path());
}
