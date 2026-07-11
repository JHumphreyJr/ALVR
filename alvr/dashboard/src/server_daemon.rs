//! Starts the always-on ALVR server daemon (Phase 2).

#[cfg(not(target_arch = "wasm32"))]
pub fn start() {
    alvr_server_core::start_server_daemon(crate::get_filesystem_layout());
}

#[cfg(target_arch = "wasm32")]
pub fn start() {}
