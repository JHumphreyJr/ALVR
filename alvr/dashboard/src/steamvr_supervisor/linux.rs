use crate::steamvr_launcher::{is_steamvr_running, maybe_kill_steamvr, LAUNCHER};
use alvr_common::{info, warn, ConnectionState};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Once,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::System;

/// First retry after 5 s, then 10, 20, 40 … capped at 5 minutes.
const BASE_BACKOFF: Duration = Duration::from_secs(5);
const MAX_BACKOFF: Duration = Duration::from_secs(300);
const HEALTH_POLL_INTERVAL: Duration = Duration::from_secs(3);
const STEAMVR_STARTUP_GRACE: Duration = Duration::from_secs(45);
const UNHEALTHY_STREAK_REQUIRED: u32 = 3;
const COMPOSITOR_LOG_TAIL_BYTES: u64 = 64 * 1024;

static SUPERVISOR_STARTED: Once = Once::new();
static MANUAL_RESTART: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Serialize, Deserialize)]
struct LaunchMarker {
    supervisor_pid: u32,
    launched_at_unix_ms: u128,
    launcher_exe: String,
}

#[derive(Default)]
struct BackoffState {
    consecutive_failures: u32,
    next_attempt_at: Option<Instant>,
}

impl BackoffState {
    fn reset(&mut self) {
        self.consecutive_failures = 0;
        self.next_attempt_at = None;
    }

    fn record_failure(&mut self) {
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        let delay = backoff_delay(self.consecutive_failures);
        self.next_attempt_at = Some(Instant::now() + delay);
        warn!(
            "SteamVR recovery backoff: attempt {} failed, next try in {:.0}s",
            self.consecutive_failures,
            delay.as_secs_f64()
        );
    }

    fn ready(&self) -> bool {
        self.next_attempt_at
            .is_none_or(|t| Instant::now() >= t)
    }
}

fn backoff_delay(failures: u32) -> Duration {
    let failures = failures.saturating_sub(1);
    let secs = BASE_BACKOFF.as_secs_f64() * 2f64.powi(failures as i32);
    Duration::from_secs_f64(secs.min(MAX_BACKOFF.as_secs_f64()))
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("alvr")
}

fn launch_marker_path() -> PathBuf {
    runtime_dir().join("steamvr-launch.json")
}

fn last_recovery_path() -> PathBuf {
    runtime_dir().join("steamvr-last-recovery.json")
}

fn now_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

pub fn write_launch_marker() {
    let _ = fs::create_dir_all(runtime_dir());
    let marker = LaunchMarker {
        supervisor_pid: std::process::id(),
        launched_at_unix_ms: now_unix_ms(),
        launcher_exe: std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    if let Ok(text) = serde_json::to_string_pretty(&marker) {
        let _ = fs::write(launch_marker_path(), text);
    }
}

fn clear_launch_marker() {
    let _ = fs::remove_file(launch_marker_path());
}

pub fn is_vrcompositor_running() -> bool {
    let system = System::new_all();
    for name in ["vrcompositor", "vrcompositor.real"] {
        if system
            .processes_by_name(OsStr::new(name))
            .next()
            .is_some()
        {
            return true;
        }
    }
    false
}

fn compositor_log_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".local/share/Steam/logs/vrcompositor.txt"))
}

fn compositor_log_tail() -> Option<String> {
    let path = compositor_log_path()?;
    let mut file = fs::File::open(path).ok()?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > COMPOSITOR_LOG_TAIL_BYTES {
        let _ = file.seek(SeekFrom::End(-(COMPOSITOR_LOG_TAIL_BYTES as i64)));
    }
    let mut buf = String::new();
    file.read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// Healthy warp mesh lines end with `shrink wrap saved 0.00%` — do not treat that as a crash.
fn compositor_log_has_healthy_warp_mesh() -> bool {
    let Some(buf) = compositor_log_tail() else {
        return false;
    };
    buf.lines().rev().take(40).any(|line| {
        line.contains("Warp mesh")
            && line.contains("covers")
            && !line.contains("covers 0.00%")
    })
}

/// Recent compositor crash signatures from SteamVR logs.
fn compositor_log_unhealthy() -> bool {
    let Some(buf) = compositor_log_tail() else {
        return false;
    };

    let recent_lines: Vec<_> = buf.lines().rev().take(80).collect();

    if recent_lines.iter().any(|line| {
        line.contains("Fail (-203)") || line.contains("BInit failed")
    }) {
        return true;
    }

    // Only match warp mesh collapse, not HiddenArea(0.00%) or "saved 0.00%".
    recent_lines.iter().any(|line| {
        line.contains("Warp mesh") && line.contains("covers 0.00%")
    })
}

fn session_path() -> PathBuf {
    crate::get_filesystem_layout().session()
}

fn client_needs_steamvr() -> bool {
    let Ok(text) = fs::read_to_string(session_path()) else {
        return false;
    };
    let Ok(session) = serde_json::from_str::<alvr_session::SessionConfig>(&text) else {
        return false;
    };
    session.client_connections.values().any(|c| {
        matches!(
            c.connection_state,
            ConnectionState::Streaming | ConnectionState::Connecting
        )
    })
}

fn driver_blocked_in_safe_mode() -> bool {
    let Ok(path) = alvr_server_io::steamvr_settings_file_path() else {
        return false;
    };
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    json.get("driver_alvr_server")
        .and_then(|d| d.get("blocked_by_safe_mode"))
        .and_then(|b| b.as_bool())
        .unwrap_or(false)
}

#[derive(Debug)]
enum HealthStatus {
    Healthy,
    NotRunning,
    CompositorMissing,
    CompositorUnhealthy,
    DriverBlocked,
    ForeignProcess,
}

fn steamvr_health() -> HealthStatus {
    if driver_blocked_in_safe_mode() {
        return HealthStatus::DriverBlocked;
    }

    if !is_steamvr_running() {
        return HealthStatus::NotRunning;
    }

    // vrcompositor often does not start until a headset is streaming; don't tear down
    // a healthy idle SteamVR while waiting for the client to connect.
    if client_needs_steamvr() && !is_vrcompositor_running() {
        return HealthStatus::CompositorMissing;
    }

    if compositor_log_unhealthy() {
        // Ignore stale crash lines while the compositor is clearly healthy now.
        if !(client_needs_steamvr() && compositor_log_has_healthy_warp_mesh()) {
            return HealthStatus::CompositorUnhealthy;
        }
    }

    if launch_marker_path().exists() {
        HealthStatus::Healthy
    } else {
        // vrserver running but not launched by us — reclaim on next recovery cycle.
        HealthStatus::ForeignProcess
    }
}

static LAST_LAUNCH: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

fn within_startup_grace() -> bool {
    LAST_LAUNCH
        .lock()
        .map(|t| t.is_some_and(|i| i.elapsed() < STEAMVR_STARTUP_GRACE))
        .unwrap_or(false)
}

fn record_launch_attempt() {
    if let Ok(mut guard) = LAST_LAUNCH.lock() {
        *guard = Some(Instant::now());
    }
}

/// Called when SteamVR is launched outside recovery (dashboard / alvr_server startup).
pub fn record_steamvr_launch() {
    record_launch_attempt();
}

fn autostart_steamvr_enabled() -> bool {
    let Ok(text) = fs::read_to_string(session_path()) else {
        return false;
    };
    let Ok(session) = serde_json::from_str::<alvr_session::SessionConfig>(&text) else {
        return false;
    };
    session
        .session_settings
        .extra
        .steamvr_launcher
        .open_close_steamvr_with_dashboard
}

fn should_supervise() -> bool {
    // Keep SteamVR warm when autostart is enabled, a client is active, or VR is already up.
    autostart_steamvr_enabled() || client_needs_steamvr() || is_steamvr_running()
}

fn try_recover(backoff: &mut BackoffState, reason: &str) -> bool {
    if !backoff.ready() {
        return false;
    }

    if MANUAL_RESTART.swap(false, Ordering::SeqCst) {
        backoff.reset();
    }

    info!("SteamVR supervisor: recovering ({reason})");
    maybe_kill_steamvr();
    thread::sleep(Duration::from_secs(2));
    clear_launch_marker();

    LAUNCHER.lock().launch_steamvr();
    write_launch_marker();
    record_launch_attempt();

    thread::sleep(Duration::from_secs(5));

    let compositor_required = client_needs_steamvr();
    let ok = is_steamvr_running()
        && (!compositor_required || is_vrcompositor_running())
        && !compositor_log_unhealthy()
        && !driver_blocked_in_safe_mode();

    if ok {
        info!("SteamVR supervisor: recovery succeeded");
        backoff.reset();
        let _ = fs::write(
            last_recovery_path(),
            format!(
                "{{\"reason\":\"{reason}\",\"at_ms\":{}}}\n",
                now_unix_ms()
            ),
        );
        true
    } else {
        backoff.record_failure();
        false
    }
}

fn stream_recovery_requested() -> Option<String> {
    let path = runtime_dir().join("stream-recovery-request.json");
    let text = fs::read_to_string(path).ok()?;
    let _ = fs::remove_file(runtime_dir().join("stream-recovery-request.json"));
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|v| v.get("reason").and_then(|r| r.as_str()).map(|s| s.to_string()))
}

fn supervisor_loop() {
    let _ = fs::create_dir_all(runtime_dir());
    info!("SteamVR supervisor started (exponential backoff up to {}s)", MAX_BACKOFF.as_secs());

    let mut backoff = BackoffState::default();
    let mut unhealthy_streak: u32 = 0;

    loop {
        thread::sleep(HEALTH_POLL_INTERVAL);

        if !should_supervise() {
            backoff.reset();
            unhealthy_streak = 0;
            continue;
        }

        if MANUAL_RESTART.swap(false, Ordering::SeqCst) {
            backoff.reset();
            unhealthy_streak = 0;
            try_recover(&mut backoff, "manual restart");
            continue;
        }

        if let Some(reason) = stream_recovery_requested() {
            backoff.reset();
            unhealthy_streak = 0;
            try_recover(&mut backoff, &format!("stream recovery ({reason})"));
            continue;
        }

        if within_startup_grace() {
            continue;
        }

        match steamvr_health() {
            HealthStatus::Healthy => {
                backoff.reset();
                unhealthy_streak = 0;
            }
            status if matches!(
                status,
                HealthStatus::NotRunning
                    | HealthStatus::CompositorMissing
                    | HealthStatus::CompositorUnhealthy
                    | HealthStatus::DriverBlocked
                    | HealthStatus::ForeignProcess
            ) =>
            {
                unhealthy_streak += 1;
                if unhealthy_streak < UNHEALTHY_STREAK_REQUIRED {
                    continue;
                }
                unhealthy_streak = 0;

                match status {
                    HealthStatus::NotRunning
                        if client_needs_steamvr() || autostart_steamvr_enabled() =>
                    {
                        try_recover(&mut backoff, "SteamVR not running");
                    }
                    HealthStatus::CompositorMissing => {
                        try_recover(&mut backoff, "vrcompositor not running");
                    }
                    HealthStatus::CompositorUnhealthy => {
                        try_recover(&mut backoff, "compositor unhealthy (log signature)");
                    }
                    HealthStatus::DriverBlocked => {
                        try_recover(&mut backoff, "ALVR driver blocked (safe mode)");
                    }
                    HealthStatus::ForeignProcess => {
                        try_recover(&mut backoff, "SteamVR not launched by ALVR supervisor");
                    }
                    _ => {}
                }
            }
            _ => {
                unhealthy_streak = 0;
            }
        }
    }
}

pub fn start_supervisor_thread() {
    SUPERVISOR_STARTED.call_once(|| {
        thread::Builder::new()
            .name("alvr-steamvr-supervisor".into())
            .spawn(supervisor_loop)
            .ok();
    });
}

/// User or dashboard requested restart — bypass current backoff wait.
pub fn request_immediate_restart() {
    MANUAL_RESTART.store(true, Ordering::SeqCst);
}
