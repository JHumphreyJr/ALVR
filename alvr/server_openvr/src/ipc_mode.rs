//! OpenVR driver operating in IPC attach mode (server daemon owns ServerCore).

use crate::bindings::*;
use crate::props;
use alvr_common::{error, once_cell::sync::Lazy, parking_lot::Mutex, BUTTON_INFO, HAND_LEFT_ID, HAND_RIGHT_ID, HAND_TRACKER_LEFT_ID, HAND_TRACKER_RIGHT_ID};
use alvr_packets::Haptics;
use alvr_server_core::driver_ipc::{DriverIpcClient, IpcMessage, IpcServerEvent, IpcViewsConfig};
use alvr_server_core::registered_button_set;
use std::{ffi::c_void, ptr, sync::Arc, thread, time::Duration};

pub static IPC_CLIENT: Lazy<Mutex<Option<Arc<DriverIpcClient>>>> = Lazy::new(|| Mutex::new(None));

static ENCODER_PARAMS: Lazy<Mutex<Option<(u64, f32)>>> = Lazy::new(|| Mutex::new(None));

pub fn install_ffi_callbacks() {
    unsafe {
        LogError = Some(alvr_server_core::alvr_error);
        LogWarn = Some(alvr_server_core::alvr_warn);
        LogInfo = Some(alvr_server_core::alvr_info);
        LogDebug = Some(alvr_server_core::alvr_dbg_server_impl);
        LogEncoder = Some(alvr_server_core::alvr_dbg_encoder);
        LogPeriodically = Some(alvr_server_core::alvr_log_periodically);
        PathStringToHash = Some(alvr_server_core::alvr_path_to_id);
        GetSerialNumber = Some(props::get_serial_number);
        SetOpenvrProps = Some(props::set_device_openvr_props);
        RegisterButtons = Some(register_buttons);
        DriverReadyIdle = Some(driver_ready_idle);
        HapticsSend = Some(send_haptics_ipc);
        SetVideoConfigNals = Some(set_video_config_nals_ipc);
        VideoSend = Some(send_video_ipc);
        GetDynamicEncoderParams = Some(get_dynamic_encoder_params_ipc);
        ReportComposed = Some(report_composed_ipc);
        ReportPresent = Some(report_present_ipc);
        WaitForVSync = Some(wait_for_vsync_ipc);
        ShutdownRuntime = Some(shutdown_ipc);
    }
}

pub fn start_ipc_client(early_hmd_initialization: bool) {
    unsafe {
        CppInit(early_hmd_initialization);
    }

    let Some(client) = DriverIpcClient::connect() else {
        error!("Failed to connect to ALVR server daemon IPC");
        return;
    };
    let client = Arc::new(client);
    *IPC_CLIENT.lock() = Some(Arc::clone(&client));

    let client_reader = Arc::clone(&client);
    client_reader.run_event_reader(move |msg| match msg {
        IpcMessage::ServerEvent(event) => dispatch_ipc_event(event),
        IpcMessage::EncoderParams {
            bitrate_bps,
            framerate,
            ..
        } => {
            *ENCODER_PARAMS.lock() = Some((bitrate_bps, framerate));
        }
        _ => {}
    });
}

fn dispatch_ipc_event(event: IpcServerEvent) {
    match event {
        IpcServerEvent::ClientConnected => unsafe {
            if InitializeStreaming() {
                RequestDriverResync();
            } else {
                ShutdownSteamvr();
            }
        },
        IpcServerEvent::ClientDisconnected => unsafe {
            DeinitializeStreaming();
        },
        IpcServerEvent::Battery {
            device_id,
            gauge_value,
            is_plugged,
        } => unsafe {
            SetBattery(device_id, gauge_value, is_plugged);
        },
        IpcServerEvent::PlayspaceSync { width, height } => unsafe {
            SetChaperoneArea(width, height);
        },
        IpcServerEvent::ViewsConfig(config) => unsafe {
            apply_views_config(config);
        },
        IpcServerEvent::RequestIdr => unsafe {
            RequestIDR();
        },
        IpcServerEvent::CaptureFrame => unsafe {
            CaptureFrame();
        },
        IpcServerEvent::ShutdownSteamvr => unsafe {
            ShutdownSteamvr();
        },
        IpcServerEvent::RestartSteamvr => unsafe {
            ShutdownSteamvr();
        },
    }
}

unsafe fn apply_views_config(config: IpcViewsConfig) {
    SetViewsConfig(FfiViewsConfig {
        fov: [
            FfiFov {
                left: config.fov[0].left,
                right: config.fov[0].right,
                up: config.fov[0].up,
                down: config.fov[0].down,
            },
            FfiFov {
                left: config.fov[1].left,
                right: config.fov[1].right,
                up: config.fov[1].up,
                down: config.fov[1].down,
            },
        ],
        ipd_m: config.ipd_m,
    });
}

extern "C" fn driver_ready_idle(set_default_chap: bool) {
    thread::spawn(move || {
        unsafe { InitOpenvrClient() };

        if set_default_chap {
            unsafe {
                SetChaperoneArea(2.0, 2.0);
            }
        }
    });
}

/// # Safety
pub unsafe extern "C" fn register_buttons(instance_ptr: *mut c_void, device_id: u64) {
    let mapped_device_id = if device_id == *HAND_TRACKER_LEFT_ID {
        *HAND_LEFT_ID
    } else if device_id == *HAND_TRACKER_RIGHT_ID {
        *HAND_RIGHT_ID
    } else {
        device_id
    };

    for button_id in registered_button_set() {
        if let Some(info) = BUTTON_INFO.get(&button_id) {
            if info.device_id == mapped_device_id {
                unsafe { RegisterButton(instance_ptr, button_id) };
            }
        } else {
            error!("Cannot register unrecognized button ID {button_id}");
        }
    }
}

extern "C" fn send_haptics_ipc(device_id: u64, duration_s: f32, frequency: f32, amplitude: f32) {
    if let Ok(duration) = Duration::try_from_secs_f32(duration_s) {
        if let Some(client) = IPC_CLIENT.lock().as_ref() {
            client.send_haptics(Haptics {
                device_id,
                duration,
                frequency,
                amplitude,
            });
        }
    }
}

extern "C" fn set_video_config_nals_ipc(buffer_ptr: *const u8, len: i32, codec: i32) {
    let codec = match codec {
        0 => 0u8,
        1 => 1u8,
        _ => 2u8,
    };

    let mut config_buffer = vec![0; len as usize];
    unsafe { ptr::copy_nonoverlapping(buffer_ptr, config_buffer.as_mut_ptr(), len as usize) };

    if let Some(client) = IPC_CLIENT.lock().as_ref() {
        client.send_video_config(config_buffer, codec);
    }
}

extern "C" fn send_video_ipc(timestamp_ns: u64, buffer_ptr: *mut u8, len: i32, is_idr: bool) {
    if let Some(client) = IPC_CLIENT.lock().as_ref() {
        let buffer = unsafe { std::slice::from_raw_parts(buffer_ptr, len as usize) };
        client.send_video_nal(timestamp_ns, buffer.to_vec(), is_idr);
    }
}

extern "C" fn get_dynamic_encoder_params_ipc() -> FfiDynamicEncoderParams {
    if let Some(client) = IPC_CLIENT.lock().as_ref() {
        client.request_encoder_params();
    }
    if let Some((bitrate_bps, framerate)) = *ENCODER_PARAMS.lock() {
        return FfiDynamicEncoderParams {
            updated: 1,
            bitrate_bps,
            framerate,
        };
    }
    FfiDynamicEncoderParams::default()
}

extern "C" fn report_composed_ipc(timestamp_ns: u64, offset_ns: u64) {
    if let Some(client) = IPC_CLIENT.lock().as_ref() {
        client.send_composed(timestamp_ns, offset_ns);
    }
}

extern "C" fn report_present_ipc(timestamp_ns: u64, offset_ns: u64) {
    if let Some(client) = IPC_CLIENT.lock().as_ref() {
        client.send_present(timestamp_ns, offset_ns);
    }
}

extern "C" fn wait_for_vsync_ipc() {
    thread::sleep(Duration::from_millis(8));
}

extern "C" fn shutdown_ipc() {
    *IPC_CLIENT.lock() = None;
}
