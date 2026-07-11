//! Always-on server daemon: handshake + web API without waiting for SteamVR.

use crate::{
    driver_ipc::{self, IpcMessage, IpcServerEvent},
    initialize_environment, init_logging, settings, stream_health, ServerCoreContext,
    ServerCoreEvent, ViewsConfig,
};
use alvr_common::info;
use std::{
    sync::{mpsc, Arc, Once},
    thread,
    time::Duration,
};

static DAEMON_STARTED: Once = Once::new();

fn ipc_event_from_core(event: &ServerCoreEvent) -> Option<IpcServerEvent> {
    match event {
        ServerCoreEvent::ClientConnected => Some(IpcServerEvent::ClientConnected),
        ServerCoreEvent::ClientDisconnected => Some(IpcServerEvent::ClientDisconnected),
        ServerCoreEvent::Battery(info) => Some(IpcServerEvent::Battery {
            device_id: info.device_id,
            gauge_value: info.gauge_value,
            is_plugged: info.is_plugged,
        }),
        ServerCoreEvent::PlayspaceSync(bounds) => Some(IpcServerEvent::PlayspaceSync {
            width: bounds.x,
            height: bounds.y,
        }),
        ServerCoreEvent::ViewsConfig(config) => Some(IpcServerEvent::ViewsConfig(
            driver_ipc::views_config_to_ipc(config),
        )),
        ServerCoreEvent::RequestIDR => Some(IpcServerEvent::RequestIdr),
        ServerCoreEvent::CaptureFrame => Some(IpcServerEvent::CaptureFrame),
        ServerCoreEvent::ShutdownPending => Some(IpcServerEvent::ShutdownSteamvr),
        ServerCoreEvent::RestartPending => Some(IpcServerEvent::RestartSteamvr),
        ServerCoreEvent::Tracking { .. }
        | ServerCoreEvent::Buttons(_)
        | ServerCoreEvent::SetOpenvrProperty { .. }
        | ServerCoreEvent::GameRenderLatencyFeedback(_) => None,
    }
}

fn install_ipc_handlers(context: Arc<ServerCoreContext>) {
    let ctx = Arc::clone(&context);
    driver_ipc::set_driver_message_handler(move |message| {
        match message {
            IpcMessage::VideoConfig { buffer, codec } => {
                let codec = match codec {
                    0 => alvr_session::CodecType::H264,
                    1 => alvr_session::CodecType::Hevc,
                    _ => alvr_session::CodecType::AV1,
                };
                ctx.set_video_config_nals(buffer, codec);
            }
            IpcMessage::VideoNal {
                timestamp_ns,
                buffer,
                is_idr,
            } => {
                ctx.send_video_nal(
                    Duration::from_nanos(timestamp_ns),
                    buffer,
                    is_idr,
                );
            }
            IpcMessage::GetEncoderParams => {
                if let Some(params) = ctx.get_dynamic_encoder_params() {
                    if let Some(server) = driver_ipc::driver_ipc_server() {
                        server.send_raw(IpcMessage::EncoderParams {
                            updated: true,
                            bitrate_bps: params.bitrate_bps as u64,
                            framerate: params.framerate,
                        });
                    }
                }
            }
            IpcMessage::Composed {
                timestamp_ns,
                offset_ns,
            } => {
                ctx.report_composed(
                    Duration::from_nanos(timestamp_ns),
                    Duration::from_nanos(offset_ns),
                );
            }
            IpcMessage::Present {
                timestamp_ns,
                offset_ns,
            } => {
                ctx.report_present(
                    Duration::from_nanos(timestamp_ns),
                    Duration::from_nanos(offset_ns),
                );
            }
            IpcMessage::Haptics(haptics) => ctx.send_haptics(haptics),
            _ => {}
        }
    });
}

fn daemon_event_loop(context: Arc<ServerCoreContext>, events_receiver: mpsc::Receiver<ServerCoreEvent>) {
    let ipc = driver_ipc::start_driver_ipc_server();
    install_ipc_handlers(Arc::clone(&context));
    stream_health::start_stream_health_monitor(Arc::clone(&context));

    context.start_connection();

    let mut last_views: Option<ViewsConfig> = None;

    loop {
        let event = match events_receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(event) => event,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        if let ServerCoreEvent::ViewsConfig(config) = &event {
            last_views = Some(config.clone());
        }

        if matches!(event, ServerCoreEvent::ClientConnected) {
            if let Some(config) = &last_views {
                ipc.send_event(IpcServerEvent::ViewsConfig(driver_ipc::views_config_to_ipc(
                    config,
                )));
            }
        }

        if let Some(ipc_event) = ipc_event_from_core(&event) {
            ipc.send_event(ipc_event);
        }
    }
}

/// Start the always-on server (handshake + :8082) in the dashboard process.
pub fn start_server_daemon(layout: alvr_filesystem::Layout) {
    DAEMON_STARTED.call_once(|| {
        initialize_environment(layout.clone());

        let log_to_disk = settings().extra.logging.log_to_disk;
        init_logging(
            log_to_disk.then(|| layout.session_log()),
            Some(layout.crash_log()),
        );

        let (context, events_receiver) = ServerCoreContext::new();
        let context = Arc::new(context);

        thread::Builder::new()
            .name("alvr-server-daemon".into())
            .spawn(move || daemon_event_loop(context, events_receiver))
            .ok();

        info!("ALVR server daemon started (handshake active before SteamVR)");
    });
}
