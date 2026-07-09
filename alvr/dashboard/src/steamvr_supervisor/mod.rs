//! Watches SteamVR health and relaunches it with exponential backoff when the compositor
//! or vrserver dies. Only ALVR-owned launches are trusted.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use linux::*;

#[cfg(not(target_os = "linux"))]
mod stub;

#[cfg(not(target_os = "linux"))]
pub use stub::*;
