//! Headless ALVR service: SteamVR supervisor with exponential backoff recovery.

#![cfg(not(target_arch = "wasm32"))]

use std::{thread, time::Duration};

fn main() {
    let (server_events_sender, _server_events_receiver) = std::sync::mpsc::channel();
    alvr_dashboard::logging_backend::init_logging(server_events_sender);

    alvr_dashboard::data_sources::clean_session();

    #[cfg(not(target_arch = "wasm32"))]
    alvr_dashboard::server_daemon::start();

    #[cfg(target_os = "linux")]
    {
        alvr_dashboard::steamvr_supervisor::start_supervisor_thread();

        if alvr_dashboard::data_sources::get_read_only_local_session()
            .settings()
            .extra
            .steamvr_launcher
            .open_close_steamvr_with_dashboard
        {
        alvr_dashboard::steamvr_launcher::LAUNCHER
            .lock()
            .launch_steamvr();
    }
    }

    alvr_common::info!("alvr_server running (SteamVR supervisor active)");

    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
