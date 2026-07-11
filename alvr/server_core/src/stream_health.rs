//! Compositor / SteamVR health while clients are streaming (Phase 4).

use crate::{driver_ipc, ServerCoreContext, SESSION_MANAGER};
use alvr_common::{warn, ConnectionState};
use std::{
    ffi::OsStr,
    fs,
    io::{Read, Seek, SeekFrom},
    sync::Arc,
    thread,
    time::Duration,
};
use sysinfo::System;

const POLL_INTERVAL: Duration = Duration::from_secs(3);
const COMPOSITOR_ABSENT_GRACE: Duration = Duration::from_secs(5);
const COMPOSITOR_LOG_TAIL_BYTES: u64 = 64 * 1024;

fn compositor_log_path() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| {
        std::path::PathBuf::from(h)
            .join(".local/share/Steam/logs/vrcompositor.txt")
    })
}

fn compositor_log_unhealthy() -> bool {
    let Some(path) = compositor_log_path() else {
        return false;
    };
    let Ok(mut file) = fs::File::open(path) else {
        return false;
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > COMPOSITOR_LOG_TAIL_BYTES {
        let _ = file.seek(SeekFrom::End(-(COMPOSITOR_LOG_TAIL_BYTES as i64)));
    }
    let mut buf = String::new();
    if file.read_to_string(&mut buf).is_err() {
        return false;
    }
    buf.lines().rev().take(80).any(|line| {
        line.contains("Warp mesh") && line.contains("covers 0.00%")
            || line.contains("Fail (-203)")
            || line.contains("BInit failed")
    })
}

pub fn is_vrcompositor_running() -> bool {
    let system = System::new_all();
    ["vrcompositor", "vrcompositor.real"]
        .into_iter()
        .any(|name| system.processes_by_name(OsStr::new(name)).next().is_some())
}

pub fn is_vrserver_running() -> bool {
    System::new_all()
        .processes_by_name(OsStr::new("vrserver"))
        .next()
        .is_some()
}

fn any_client_streaming() -> bool {
    SESSION_MANAGER
        .read()
        .client_list()
        .values()
        .any(|c| c.connection_state == ConnectionState::Streaming)
}

pub fn start_stream_health_monitor(context: Arc<ServerCoreContext>) {
    thread::Builder::new()
        .name("alvr-stream-health".into())
        .spawn(move || {
            let mut compositor_missing_since: Option<std::time::Instant> = None;

            loop {
                thread::sleep(POLL_INTERVAL);

                if !any_client_streaming() {
                    compositor_missing_since = None;
                    continue;
                }

                let compositor_ok =
                    is_vrcompositor_running() && is_vrserver_running() && !compositor_log_unhealthy();

                if compositor_ok {
                    compositor_missing_since = None;
                    continue;
                }

                let first_seen =
                    compositor_missing_since.get_or_insert_with(std::time::Instant::now);
                if first_seen.elapsed() < COMPOSITOR_ABSENT_GRACE {
                    continue;
                }

                warn!("Compositor/SteamVR unhealthy while streaming — forcing client reconnect");
                context.request_stream_teardown();
                compositor_missing_since = None;
            }
        })
        .ok();
}

/// Ask supervisor (via marker file) to restart SteamVR after compositor loss.
pub fn request_supervisor_recovery(reason: &str) {
    let path = driver_ipc::runtime_dir().join("stream-recovery-request.json");
    let _ = fs::create_dir_all(driver_ipc::runtime_dir());
    let _ = fs::write(
        path,
        format!(
            "{{\"reason\":\"{reason}\",\"at_ms\":{}}}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ),
    );
}
