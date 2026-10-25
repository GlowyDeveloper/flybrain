use crate::{
    framebuffer::{FrameBuffer, SharedFrame},
    input::InputEvent,
    memory_bridge::init_ipc_worker,
    state::{ClientState, State},
    window::{App, UserEvent},
};
use smithay::{
    input::{SeatState, keyboard::XkbConfig, pointer::CursorImageStatus},
    output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
    reexports::{
        calloop::EventLoop as CalloopLoop,
        wayland_server::{Display, ListeningSocket, protocol::wl_shm::Format},
    },
    utils::Transform,
    wayland::{
        compositor::{CompositorClientState, CompositorState},
        selection::data_device::DataDeviceState,
        shell::xdg::{XdgShellState, decoration::XdgDecorationState},
        shm::ShmState,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        xwayland_shell::XWaylandShellState,
    },
    xwayland::{X11Wm, XWayland, XWaylandEvent},
};
use std::{
    collections::HashMap,
    process::{Command, Stdio},
    sync::{Arc, Mutex, mpsc},
};
use tracing::{Level, debug, error, info, warn};
use winit::event_loop::EventLoop;

pub mod framebuffer;
pub mod input;
pub mod memory_bridge;
pub mod python;
pub mod state;
pub mod window;

pub const HEADER: &str = "\x1b[95m";
pub const GREEN: &str = "\x1b[92m";
pub const ENDC: &str = "\x1b[0m";

pub static PYTHON: Mutex<&str> = Mutex::new("");
pub static VERBOSITY: Mutex<Verbosity> = Mutex::new(Verbosity::Normal);
pub static WIDTH: Mutex<i32> = Mutex::new(1280);
pub static HEIGHT: Mutex<i32> = Mutex::new(720);
pub static HZ: Mutex<i32> = Mutex::new(60);
pub static FULLSCREEN: Mutex<bool> = Mutex::new(false);
pub static WINDOW: Mutex<Option<String>> = Mutex::new(None);

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum Verbosity {
    Verbose,
    VeryVerbose,
    Normal,
}

fn help() {
    println!();
    println!("{HEADER}Usage:{ENDC} flybrain [options]");
    println!();
    println!("{HEADER}Options:");
    println!(" {GREEN}-width <width>{ENDC}   Set the width of the output (default: 1280)");
    println!(" {GREEN}-height <height>{ENDC} Set the height of the output (default: 720)");
    println!(" {GREEN}-hz <hz>{ENDC}         Set the refresh rate of the output (default: 60)");
    println!(" {GREEN}-window <window>{ENDC} Only display the output in the specified window");
    println!(" {GREEN}-fullscreen{ENDC}      Forces the window to be fullscreened at all times");
    println!(" {GREEN}-v{ENDC}               Launch the application in verbose mode");
    println!(" {GREEN}-vv{ENDC}              Launch the application in very verbose mode");
    println!(" {GREEN}--{ENDC}               Used to launch the application");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        help();
        return Ok(());
    }

    #[allow(unused_mut)]
    let mut split;

    let cmd = if let Some(cmd) = args.iter().position(|arg| arg == "--") {
        let (_, args) = args.split_at(cmd + 1);
        split = cmd;
        args.to_vec()
    } else {
        help();
        return Ok(());
    };

    let (left, _) = args.split_at(split);

    if left.contains(&"-vv".to_string()) {
        *VERBOSITY.lock().unwrap() = Verbosity::VeryVerbose;
    } else if left.contains(&"-v".to_string()) {
        *VERBOSITY.lock().unwrap() = Verbosity::Verbose;
    }

    if left.contains(&"-fullscreen".to_string()) {
        *FULLSCREEN.lock().unwrap() = true;
    }

    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(if *VERBOSITY.lock().unwrap() == Verbosity::VeryVerbose {
            Level::TRACE
        } else if *VERBOSITY.lock().unwrap() == Verbosity::Verbose {
            Level::INFO
        } else {
            Level::WARN
        })
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    *PYTHON.lock().unwrap() = python::get_python();
    python::install_pip_dependencies()?;
    python::install_synapses()?;

    let width = if let Some(width) = args.iter().position(|arg| arg == "-width") {
        let (_, args) = args.split_at(width + 1);
        args[0].parse::<i32>().unwrap_or(1024)
    } else {
        1280
    };

    let height = if let Some(height) = args.iter().position(|arg| arg == "-height") {
        let (_, args) = args.split_at(height + 1);
        args[0].parse::<i32>().unwrap_or(1024)
    } else {
        720
    };

    let hz = if let Some(hz) = args.iter().position(|arg| arg == "-hz") {
        let (_, args) = args.split_at(hz + 1);
        args[0].parse::<i32>().unwrap_or(60)
    } else {
        60
    };

    let window = if let Some(window) = args.iter().position(|arg| arg == "-window") {
        let (_, args) = args.split_at(window + 1);
        let mut window = args[0].clone();
        if window.starts_with("\"") {
            window = window.strip_prefix("\"").unwrap().to_string();
        }
        if window.ends_with("\"") {
            window = window.strip_suffix("\"").unwrap().to_string();
        }
        Some(window)
    } else {
        None
    };

    *WIDTH.lock().unwrap() = width;
    *HEIGHT.lock().unwrap() = height;
    *HZ.lock().unwrap() = hz;
    *WINDOW.lock().unwrap() = window;

    debug!("cmd: {:?}", cmd);
    debug!("width: {}", width);
    debug!("height: {}", height);
    debug!("hz: {}", hz);

    let mut display: Display<State> = Display::new()?;
    let display_handle = display.handle();

    let compositor_state = CompositorState::new::<State>(&display.handle());

    let xdg_shell_state = XdgShellState::new::<State>(&display.handle());
    let xdg_decoration_state = XdgDecorationState::new::<State>(&display.handle());

    let mut seat_state = SeatState::new();

    let shm_state = ShmState::new::<State>(&display.handle(), [Format::Xrgb8888]);

    let mut seat = seat_state.new_wl_seat(&display.handle(), "seat0");
    seat.add_keyboard(XkbConfig::default(), 200, 25)?;
    seat.add_pointer();

    let data_device_state = DataDeviceState::new::<State>(&display.handle());

    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let shared_frame: SharedFrame = Arc::new(Mutex::new(FrameBuffer::new()));
    let viewporter_state = ViewporterState::new::<State>(&display.handle());
    let text_input_state = TextInputManagerState::new::<State>(&display.handle());

    let xwayland_shell_state = XWaylandShellState::new::<State>(&display.handle());

    let mut state = State {
        compositor_state,
        xdg_shell_state,
        xdg_decoration_state,
        seat_state,
        seat,
        shm_state,
        data_device_state,
        viewporter_state,
        text_input_state,

        xwayland_shell_state,
        xwms: HashMap::new(),

        windows: Vec::new(),
        x11_windows: Vec::new(),
        active_surface: None,
        cursor: CursorImageStatus::default_named(),
        shared_frame: shared_frame.clone(),
        proxy,
    };

    info!("Created seat");

    let socket = ListeningSocket::bind("flybrain")?;

    info!("Listening on WAYLAND_DISPLAY=flybrain");

    let output = Output::new(
        "flybrain".to_string(),
        PhysicalProperties {
            size: (200, 150).into(),
            subpixel: Subpixel::Unknown,
            make: "flybrain".into(),
            model: "Virtual Display".into(),
            serial_number: "".into(),
        },
    );

    let _output_global = output.create_global::<State>(&display.handle());

    output.change_current_state(
        Some(Mode {
            size: (width, height).into(),
            refresh: hz,
        }),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );

    output.set_preferred(Mode {
        size: (width, height).into(),
        refresh: hz,
    });

    info!("Created output");

    let (display_tx, display_rx) = mpsc::channel::<Option<u32>>();

    std::thread::Builder::new()
        .name("launcher".to_string())
        .spawn(move || {
            launch_app(cmd, display_rx).map_or_else(
                |e| {
                    error!(e);
                    std::process::exit(1)
                },
                |code| std::process::exit(code),
            );
        })?;

    info!("Created launcher thread");

    python::run_main_python().map_or_else(
        |e| {
            error!(e);
            std::process::exit(1)
        },
        |_| {},
    );

    let (input_tx, input_rx) = mpsc::channel::<InputEvent>();

    init_ipc_worker(input_tx.clone());

    let wayland_core_engine_display_handle = display_handle.clone();
    std::thread::Builder::new()
        .name("wayland_core_engine".to_string())
        .spawn(move || {
            let mut event_loop: CalloopLoop<State> =
                CalloopLoop::try_new().expect("Failed to create calloop event loop");
            let loop_handle = event_loop.handle();

            match XWayland::spawn(
                &wayland_core_engine_display_handle,
                None,
                [("XWAYLAND_NO_GLAMOR", "1")],
                std::iter::empty::<&str>(),
                false,
                Stdio::inherit(),
                Stdio::inherit(),
                |_| {},
            ) {
                Ok((xwayland, client)) => {
                    let wm_handle = loop_handle.clone();
                    let display_tx_err = display_tx.clone();
                    loop_handle
                        .insert_source(xwayland, move |event, _, state| match event {
                            XWaylandEvent::Ready {
                                x11_socket,
                                display_number,
                            } => {
                                match X11Wm::start_wm(
                                    wm_handle.clone(),
                                    &wayland_core_engine_display_handle,
                                    x11_socket,
                                    client.clone(),
                                ) {
                                    Ok(wm) => {
                                        info!("XWayland ready on DISPLAY=:{}", display_number);
                                        state.xwms.insert(wm.id(), wm);
                                        let _ = display_tx.send(Some(display_number));
                                    }
                                    Err(e) => {
                                        error!("Failed to start X11 WM: {}", e);
                                        let _ = display_tx.send(None);
                                    }
                                }
                            }
                            XWaylandEvent::Error => {
                                error!("XWayland failed to start");
                                let _ = display_tx_err.send(None);
                            }
                        })
                        .expect("Failed to insert XWayland event source");
                }
                Err(e) => {
                    error!("Failed to spawn XWayland: {}", e);
                    let _ = display_tx.send(None);
                }
            }

            loop {
                while let Ok(event) = input_rx.try_recv() {
                    state.process_input_event(event);
                }

                match socket.accept() {
                    Ok(Some(stream)) => {
                        info!("New client connecting...");
                        let _ = display
                            .handle()
                            .insert_client(
                                stream,
                                Arc::new(ClientState {
                                    compositor_state: CompositorClientState::default(),
                                }),
                            )
                            .map_err(|e| error!("Failed to insert client: {}", e));
                    }
                    Ok(None) => {}
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => error!("Socket error: {}", e),
                }

                let _ = display
                    .flush_clients()
                    .map_err(|e| error!("Flush error: {}", e));

                let _ = display
                    .dispatch_clients(&mut state)
                    .map_err(|e| error!("Dispatch error: {}", e));

                if let Err(e) =
                    event_loop.dispatch(Some(std::time::Duration::from_millis(1)), &mut state)
                {
                    error!("calloop dispatch error: {}", e);
                }
            }
        })?;

    info!("Created wayland_core_engine thread");

    let mut app = App::new(shared_frame, input_tx);
    event_loop.run_app(&mut app)?;

    Ok(())
}

fn launch_app(
    cmd: Vec<String>,
    display_rx: mpsc::Receiver<Option<u32>>,
) -> Result<i32, Box<dyn std::error::Error>> {
    if cmd.is_empty() {
        return Err("no command specified".into());
    }

    let mut envs = Vec::new();
    let mut executable = None;
    let mut args = Vec::new();

    for item in cmd {
        if executable.is_none() {
            if let Some((key, value)) = item.split_once("=")
                && !key.is_empty()
            {
                envs.push((key.to_owned(), value.to_owned()));
                continue;
            }
            executable = Some(item);
        } else {
            args.push(item);
        }
    }

    let executable = executable.ok_or("no executable specified")?;
    let mut command = Command::new(executable.clone());
    command.args(args);

    for (key, value) in envs {
        command.env(key, value);
    }

    if let Ok(host_runtime) = std::env::var("XDG_RUNTIME_DIR") {
        command.env("XDG_RUNTIME_DIR", &host_runtime);
        command.env(
            "PRESSURE_VESSEL_FILESYSTEMS_RW",
            format!("{}/flybrain", host_runtime),
        );
    }
    command.env("WAYLAND_DISPLAY", "flybrain");

    match display_rx.recv_timeout(std::time::Duration::from_secs(10)) {
        Ok(Some(n)) => {
            command.env("DISPLAY", format!(":{n}"));
        }
        _ => {
            warn!("XWayland not available; launching without DISPLAY");
            command.env_remove("DISPLAY");
        }
    }

    let display_env = command
        .get_envs()
        .find(|(k, _)| k.to_str() == Some("DISPLAY"))
        .and_then(|(_, v)| v.map(|v| v.to_string_lossy().into_owned()));
    info!("Launching {} with DISPLAY={:?}", executable, display_env);

    if *VERBOSITY.lock().unwrap() == Verbosity::VeryVerbose {
        command.env("WAYLAND_DEBUG", "1");
        command.env("PRESSURE_VESSEL_ENV_PASSTHROUGH", "WAYLAND_DEBUG");
    }

    info!("Launching {}", executable);
    debug!("Full command: {:?}", command);
    let status = command.status()?;
    info!("Executable finished");

    Ok(status.code().unwrap_or(1))
}
