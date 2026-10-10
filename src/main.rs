use std::sync::Arc;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm, delegate_xdg_shell, input::{Seat, SeatHandler, SeatState, keyboard::XkbConfig, pointer::CursorImageStatus}, output::{
        Mode, Output, PhysicalProperties, Scale, Subpixel
    }, reexports::wayland_server::{
        Client, Display, ListeningSocket, Resource,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{
            wl_buffer::WlBuffer, wl_seat::WlSeat,
            wl_shm::Format, wl_surface::WlSurface,
        },
    }, utils::{Serial, Transform}, wayland::{
        buffer::BufferHandler, compositor::{BufferAssignment, CompositorClientState, CompositorHandler, CompositorState, SurfaceAttributes, with_states}, output::OutputHandler, selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        }, shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        }, shm::{ShmHandler, ShmState, with_buffer_contents},
    }
};

struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        println!("Client connected: {:?}", client_id);
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        println!("Client disconnected: {:?}, reason: {:?}", client_id, reason);
    }
}

struct State {
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    seat_state: SeatState<State>,
    _seat: Seat<State>,
    shm_state: ShmState,
    data_device_state: DataDeviceState,

    windows: Vec<ToplevelSurface>,
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        println!("Surface committed: {:?}", surface.id());

        let buffer = with_states(surface, |states| {
            let mut attributes = states
                .cached_state
                .get::<SurfaceAttributes>();
            
            let attributes = attributes.current();
    
            match &attributes.buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer.clone()),
                _ => None,
            }
        });

        if let Some(buffer) = buffer {
            with_buffer_contents(&buffer, |ptr, len, metadata| {
                println!(
                    "Got buffer: ptr={ptr:p}, len={len}, metadata={metadata:?}"
                );
            }).unwrap();
        }
    }
}

delegate_compositor!(State);

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        println!("New toplevel: {:?}", surface.wl_surface().id());
    
        surface.send_configure();
    
        self.windows.push(surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        println!("New popup: {:?}", surface.wl_surface().id());
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        println!("Popup grab");
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        println!("Popup reposition request");
    }
}

delegate_xdg_shell!(State);

impl SeatHandler for State {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, _image: CursorImageStatus) {}

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&WlSurface>) {}
}

delegate_seat!(State);

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

delegate_shm!(State);

impl DataDeviceHandler for State {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

delegate_data_device!(State);

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl ClientDndGrabHandler for State {}
impl ServerDndGrabHandler for State {}

impl OutputHandler for State {}

delegate_output!(State);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let mut display: Display<State> = Display::new()?;

    let compositor_state = CompositorState::new::<State>(&display.handle());

    let xdg_shell_state = XdgShellState::new::<State>(&display.handle());

    let mut seat_state = SeatState::new();

    let shm_state = ShmState::new::<State>(&display.handle(), [Format::Xrgb8888]);

    let mut seat = seat_state.new_wl_seat(&display.handle(), "seat0");
    seat.add_keyboard(XkbConfig::default(), 200, 25)?;
    seat.add_pointer();

    let data_device_state = DataDeviceState::new::<State>(&display.handle());

    let mut state = State {
        compositor_state,
        xdg_shell_state,
        seat_state,
        _seat: seat,
        shm_state,
        data_device_state,

        windows: Vec::new(),
    };

    println!("Created seat");

    let socket = ListeningSocket::bind("flybrain")?;

    println!("Listening on WAYLAND_DISPLAY=flybrain");

    let output = Output::new(
        "RenderThing".to_string(),
        PhysicalProperties {
            size: (200, 150).into(),
            subpixel: Subpixel::Unknown,
            make: "flybrain".into(),
            model: "Virtual Display".into(),
        },
    );
    
    let _output_global = output.create_global::<State>(&display.handle());
    
    output.change_current_state(
        Some(Mode {
            size: (1920, 1080).into(),
            refresh: 60,
        }),
        Some(Transform::Normal),
        Some(Scale::Integer(1)),
        Some((0, 0).into()),
    );
    
    output.set_preferred(Mode {
        size: (1920, 1080).into(),
        refresh: 60,
    });

    loop {
        if let Some(stream) = socket.accept()? {
            println!("New client");

            display.handle().insert_client(
                stream,
                Arc::new(ClientState {
                    compositor_state: CompositorClientState::default(),
                }),
            )?;
        }

        display.dispatch_clients(&mut state)?;
        display.flush_clients()?;
    }
}
