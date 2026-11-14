use crate::{
    framebuffer::{FrameBuffer, SharedFrame},
    input::InputEvent,
    memory_bridge::init_ipc_worker,
    settings::{
        initilize_tomls,
        structs::{CurrentToml, EnvToml, SaveToml},
    },
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
    sync::{Arc, LazyLock, Mutex, mpsc},
};
use tracing::{Level, debug, error, info, warn};
use winit::event_loop::EventLoop;

pub mod framebuffer;
pub mod input;
pub mod memory_bridge;
pub mod python;
pub mod settings;
pub mod state;
pub mod window;

pub const HEADER: &str = "\x1b[95m";
pub const GREEN: &str = "\x1b[92m";
pub const ENDC: &str = "\x1b[0m";

pub static PYTHON: Mutex<&str> = Mutex::new("");
pub static VERBOSITY: Mutex<Verbosity> = Mutex::new(Verbosity::Normal);
pub static SAVE: LazyLock<Mutex<CurrentToml>> =
    LazyLock::new(|| Mutex::new(CurrentToml::default()));
pub static ENV: LazyLock<Mutex<EnvToml>> = LazyLock::new(|| Mutex::new(EnvToml::default()));
pub static NAME: Mutex<String> = Mutex::new(String::new());

#[derive(Debug, Clone, PartialEq, Copy)]
pub enum Verbosity {
    Verbose,
    VeryVerbose,
    Normal,
}

fn help() {
    println!();
    println!("{HEADER}Usage:{ENDC}");
    println!("    {GREEN}flybrain <name> [flags]{ENDC}");
    println!("        Relaunch an existing save.");
    println!("    {GREEN}flybrain <name> [options] [flags] -- <application>{ENDC}");
    println!("        Create or update a save named <name>, then launch the application.");
    println!("    {GREEN}flybrain [options] [flags] -- <application>{ENDC}");
    println!("        Runs the application without creating a save.");
    println!();
    println!("{HEADER}Options:{ENDC}");
    println!("    {GREEN}-width <pixels>{ENDC}   Set the output width (default: 1280).");
    println!("    {GREEN}-height <pixels>{ENDC}  Set the output height (default: 720).");
    println!("    {GREEN}-hz <rate>{ENDC}        Set the output refresh rate in Hz (default: 60).");
    println!("    {GREEN}-window <title>{ENDC}   Display output in the specified window.");
    println!("    {GREEN}-fullscreen{ENDC}       Force fullscreen mode.");
    println!("    {GREEN}-quiet{ENDC}            Suppress application logs.");
    println!("    {GREEN}-inputs [inputs]{ENDC}  The buttons that can be pressed.");
    println!();
    println!("{HEADER}Flags:{ENDC}                Flags are not saved to a save's toml file.");
    println!("    {GREEN}-v{ENDC}                Enable verbose logging.");
    println!("    {GREEN}-vv{ENDC}               Enable very verbose logging.");
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        help();
        return Ok(());
    }

    initilize_tomls();

    let name = &args[1];

    let left = if !name.starts_with("-") && args.len() == 2 {
        args.clone()
    } else {
        if let Some(cmd) = args.iter().position(|arg| arg == "--") {
            let (left, _) = args.split_at(cmd);
            left.to_vec()
        } else {
            args.clone()
        }
    };

    if left.contains(&"-vv".to_string()) {
        *VERBOSITY.lock().unwrap() = Verbosity::VeryVerbose;
    } else if left.contains(&"-v".to_string()) {
        *VERBOSITY.lock().unwrap() = Verbosity::Verbose;
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

    settings::initilize_tomls();

    let toml = if name.starts_with("-") {
        if let Some(t) = settings::launch_without_save(args.clone()) {
            settings::write_policy(None);
            t
        } else {
            help();
            return Ok(());
        }
    } else if args.iter().any(|a| a == "--") {
        if let Some(t) = settings::update_or_create_save(name.clone(), args.clone()) {
            settings::write_policy(Some(name.clone()));
            t
        } else {
            help();
            return Ok(());
        }
    } else {
        settings::write_policy(Some(name.clone()));
        settings::get_created(name.clone())
    };

    let current = settings::write_current(&toml);

    *SAVE.lock().unwrap() = current.clone();
    *NAME.lock().unwrap() = name.clone();
    *ENV.lock().unwrap() = settings::get_env();

    *PYTHON.lock().unwrap() = python::get_python();
    python::install_pip_dependencies()?;
    python::install_synapses()?;

    start(toml)
}

fn save_policy() {
    let current = settings::dot_flybrain().join("current.npz");
    let policy = settings::dot_flybrain()
        .join("saves")
        .join(NAME.lock().unwrap().clone())
        .join("policy.npz");

    let before = std::fs::metadata(&policy).unwrap().len();
    let expected = std::fs::metadata(&current).unwrap().len();

    let copied = std::fs::copy(&current, &policy);

    let after = std::fs::metadata(&policy).unwrap().len();

    debug!(
        "before: {} expected: {} copied: {:?} after: {}",
        before, expected, copied, after
    );

    if let Ok(copied) = copied {
        if !after == copied {
            error!("{:#?} couldn't be copied to {:#?}", current, policy);
        }
    } else {
        error!(
            "{:#?} couldn't be copied to {:#?}: {:#?}",
            current, policy, copied
        );
    }
}

fn start(toml: SaveToml) -> Result<(), Box<dyn std::error::Error>> {
    ctrlc::set_handler(move || {
        save_policy();
        std::process::exit(130);
    })
    .expect("Error setting Ctrl-C handler");

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
            size: (toml.options.width, toml.options.height).into(),
            refresh: toml.options.hz,
        }),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );

    output.set_preferred(Mode {
        size: (toml.options.width, toml.options.height).into(),
        refresh: toml.options.hz,
    });

    info!("Created output");

    let (display_tx, display_rx) = mpsc::channel::<Option<u32>>();

    std::thread::Builder::new()
        .name("launcher".to_string())
        .spawn(move || {
            launch_app(display_rx).map_or_else(
                |e| {
                    error!(e);
                    save_policy();
                    std::process::exit(1)
                },
                |code| {
                    save_policy();
                    std::process::exit(code)
                },
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

fn launch_app(display_rx: mpsc::Receiver<Option<u32>>) -> Result<i32, Box<dyn std::error::Error>> {
    let toml = SAVE.lock().unwrap().clone();

    let mut command = Command::new(toml.application.executable.clone());
    command.args(toml.application.args);

    for (key, value) in toml.application.env {
        command.env(key, value);
    }

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
    info!(
        "Launching {} with DISPLAY={:?}",
        toml.application.executable, display_env
    );

    if *VERBOSITY.lock().unwrap() == Verbosity::VeryVerbose {
        command.env("WAYLAND_DEBUG", "1");
        command.env("PRESSURE_VESSEL_ENV_PASSTHROUGH", "WAYLAND_DEBUG");
    }

    info!("Launching {}", toml.application.executable);
    debug!("Full command: {:?}", command);
    if toml.options.quiet {
        Ok(command.output()?.status.code().unwrap_or(1))
    } else {
        Ok(command.status()?.code().unwrap_or(1))
    }
}
