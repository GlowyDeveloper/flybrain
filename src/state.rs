use std::time::{SystemTime, UNIX_EPOCH};

use crate::{framebuffer::SharedFrame, input::InputEvent, window::UserEvent};
use smithay::{
    backend::input::{Axis, ButtonState, KeyState},
    delegate_compositor, delegate_data_device, delegate_output, delegate_seat, delegate_shm,
    delegate_xdg_shell,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::FilterResult,
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent},
    },
    reexports::wayland_server::{
        Client, Resource,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::{wl_buffer::WlBuffer, wl_seat::WlSeat, wl_surface::WlSurface},
    },
    utils::{SERIAL_COUNTER, Serial},
    wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction, get_parent, with_states,
            with_surface_tree_downward,
        },
        output::OutputHandler,
        selection::{
            SelectionHandler,
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState, with_buffer_contents},
    },
};
use tracing::{info, trace};
use winit::event_loop::EventLoopProxy;

pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        info!("Client connected: {:?}", client_id);
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        info!("Client disconnected: {:?}, reason: {:?}", client_id, reason);
    }
}

pub struct State {
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub seat_state: SeatState<State>,
    pub seat: Seat<State>,
    pub shm_state: ShmState,
    pub data_device_state: DataDeviceState,

    pub windows: Vec<ToplevelSurface>,
    pub cursor: CursorImageStatus,

    pub shared_frame: SharedFrame,
    pub proxy: EventLoopProxy<UserEvent>,
}

impl State {
    pub fn process_input_event(&mut self, event: InputEvent) {
        match event {
            InputEvent::Keyboard {
                keycode,
                pressed,
                time,
            } => {
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(keyboard) = self.seat.get_keyboard() {
                    keyboard.input::<(), _>(
                        self,
                        (keycode + 8).into(),
                        if pressed {
                            KeyState::Pressed
                        } else {
                            KeyState::Released
                        },
                        serial,
                        time,
                        |_, _, _| FilterResult::Forward,
                    );
                }
            }
            InputEvent::PointerMotion { x, y, time } => {
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(pointer) = self.seat.get_pointer() {
                    let focus = self
                        .windows
                        .first()
                        .map(|w| (w.wl_surface().clone(), (0.0, 0.0).into()));

                    pointer.motion(
                        self,
                        focus,
                        &MotionEvent {
                            location: (x, y).into(),
                            serial,
                            time,
                        },
                    );
                    pointer.frame(self);
                }
            }
            InputEvent::PointerButton {
                button,
                pressed,
                time,
            } => {
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(pointer) = self.seat.get_pointer() {
                    pointer.button(
                        self,
                        &ButtonEvent {
                            button,
                            state: if pressed {
                                ButtonState::Pressed
                            } else {
                                ButtonState::Released
                            },
                            serial,
                            time,
                        },
                    );
                    pointer.frame(self);
                }
            }
            InputEvent::PointerAxis {
                horizontal,
                vertical,
                time,
            } => {
                if let Some(pointer) = self.seat.get_pointer() {
                    let frame = AxisFrame::new(time)
                        .value(Axis::Horizontal, horizontal)
                        .value(Axis::Vertical, vertical);
                    pointer.axis(self, frame);
                    pointer.frame(self);
                }
            }
            InputEvent::CloseRequest => {
                for window in &self.windows {
                    window.send_close();
                }
            }
            InputEvent::Resize { height, width } => {
                for window in &self.windows {
                    window.with_pending_state(|state| {
                        state.size = Some((width as i32, height as i32).into());
                    });

                    window.send_configure();
                }
            }
        }
    }

    fn root_surface(surface: &WlSurface) -> WlSurface {
        let mut current = surface.clone();
        while let Some(parent) = get_parent(&current) {
            current = parent;
        }
        current
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        let root = Self::root_surface(surface);

        trace!(
            "commit: surface={:?} root={:?} known_toplevels={:?}",
            surface.id(),
            root.id(),
            self.windows
                .iter()
                .map(|w| w.wl_surface().id())
                .collect::<Vec<_>>()
        );

        let is_toplevel_surface = self.windows.iter().any(|w| *w.wl_surface() == root);
        if !is_toplevel_surface {
            trace!("commit: rejected, root not in known toplevels");
            return;
        }

        let buffer = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let attributes = attributes.current();
            match &attributes.buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer.clone()),
                Some(BufferAssignment::Removed) => None,
                None => None,
            }
        });
        if let Some(buffer) = buffer {
            with_buffer_contents(&buffer, |ptr, len, metadata| {
                trace!("New buffer content");
                let (width, height, stride) = (metadata.width, metadata.height, metadata.stride);
                let mut frame = self.shared_frame.lock().unwrap();
                frame.width = width as u32;
                frame.height = height as u32;
                frame.pixels.resize((width * height) as usize, 0);

                let src = unsafe { std::slice::from_raw_parts(ptr, len) };
                for y in 0..height as usize {
                    let row_start = y * stride as usize;
                    let row = &src[row_start..row_start + width as usize * 4];
                    for x in 0..width as usize {
                        let b = row[x * 4] as u32;
                        let g = row[x * 4 + 1] as u32;
                        let r = row[x * 4 + 2] as u32;
                        frame.pixels[y * width as usize + x] = (r << 16) | (g << 8) | b;
                    }
                }
            })
            .unwrap();

            self.proxy
                .send_event(UserEvent::FrameReady(self.cursor.clone()))
                .ok();
        }

        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u32;

        with_surface_tree_downward(
            &root,
            (),
            |_, _, _| TraversalAction::DoChildren(()),
            |_surf, states, _| {
                let mut attrs = states.cached_state.get::<SurfaceAttributes>();
                let attrs = attrs.current();
                for callback in attrs.frame_callbacks.drain(..) {
                    callback.done(time);
                }
            },
            |_, _, _| true,
        );
    }
}

delegate_compositor!(State);

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        info!("New toplevel: {:?}", surface.wl_surface().id());
        surface.send_configure();

        let serial = SERIAL_COUNTER.next_serial();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, Some(surface.wl_surface().clone()), serial);
        }

        self.windows.push(surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, _positioner: PositionerState) {
        info!("New popup: {:?}", surface.wl_surface().id());
    }

    fn grab(&mut self, _surface: PopupSurface, _seat: WlSeat, _serial: Serial) {
        info!("Popup grab");
    }

    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
        info!("Popup reposition request");
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

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor = image;
    }

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
