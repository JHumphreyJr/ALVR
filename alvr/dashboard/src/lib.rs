#[cfg(not(target_arch = "wasm32"))]
pub mod data_sources;
#[cfg(target_arch = "wasm32")]
pub mod data_sources_wasm;

pub mod dashboard;

#[cfg(not(target_arch = "wasm32"))]
pub mod server_daemon;
#[cfg(not(target_arch = "wasm32"))]
pub mod logging_backend;
#[cfg(not(target_arch = "wasm32"))]
pub mod steamvr_launcher;
#[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
pub mod steamvr_supervisor;

#[cfg(not(target_arch = "wasm32"))]
pub use data_sources::DataSources;
#[cfg(target_arch = "wasm32")]
pub use data_sources_wasm::DataSources;

use alvr_filesystem as afs;

pub fn filesystem_layout() -> afs::Layout {
    afs::filesystem_layout_from_dashboard_exe(&std::env::current_exe().unwrap()).unwrap()
}

pub fn get_filesystem_layout() -> afs::Layout {
    filesystem_layout()
}
