use crate::{HEIGHT, WIDTH, framebuffer::SharedFrame, input::InputEvent, window::UserEvent};
use smithay::{
    backend::input::{Axis, ButtonState, InputTime, KeyState}, delegate_dispatch2, input::{
        Seat, SeatHandler, SeatState,
        dnd::DndGrabHandler,
        keyboard::{FilterResult, KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent},
    }, reexports::{
        wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode,
        wayland_server::{
            Client, Resource,
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{
                wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
            },
        },
    }, utils::{IsAlive, Logical, Point, Rectangle, SERIAL_COUNTER, Serial, Size}, wayland::{
        buffer::BufferHandler,
        compositor::{
            BufferAssignment, CompositorClientState, CompositorHandler, CompositorState,
            SurfaceAttributes, TraversalAction, get_parent, with_states,
            with_surface_tree_downward,
        },
        output::OutputHandler,
        pointer_constraints::PointerConstraintsHandler,
        seat::WaylandFocus,
        selection::{
            SelectionHandler,
            data_device::{DataDeviceHandler, DataDeviceState, WaylandDndGrabHandler},
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            decoration::{XdgDecorationHandler, XdgDecorationState},
        },
        shm::{ShmHandler, ShmState, with_buffer_contents},
        text_input::{TextInputActivation, TextInputManagerState},
        viewporter::ViewporterState,
        xwayland_shell::{XWaylandShellHandler, XWaylandShellState},
    }, xwayland::{
        X11Surface, X11Wm, XWaylandClientData, XwmHandler,
        xwm::{Reorder, ResizeEdge, XwmId},
    },
};
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{info, trace, warn};
use winit::event_loop::EventLoopProxy;


#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusMode {
    Late,
    Legacy,
    Wl,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureMode {
    Auto,
    Any,
}

struct Tunables {
    focus: FocusMode,
    capture: CaptureMode,
}

fn tunables() -> &'static Tunables {
    static TUNABLES: OnceLock<Tunables> = OnceLock::new();
    TUNABLES.get_or_init(|| {
        let focus = match std::env::var("RT_FOCUS").as_deref() {
            Ok("legacy") => FocusMode::Legacy,
            Ok("wl") => FocusMode::Wl,
            Ok("off") => FocusMode::Off,
            _ => FocusMode::Late,
        };
        let capture = match std::env::var("RT_CAPTURE").as_deref() {
            Ok("any") => CaptureMode::Any,
            _ => CaptureMode::Auto,
        };
        info!("tunables: RT_FOCUS={:?} RT_CAPTURE={:?}", focus, capture);
        Tunables { focus, capture }
    })
}

static ACTIVE_COMMITS: AtomicU64 = AtomicU64::new(0);
static ACTIVE_BUFFERS: AtomicU64 = AtomicU64::new(0);
static CAPTURED: AtomicU64 = AtomicU64::new(0);
static LAST_REPORT_MS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyboardFocusTarget {
    Wl(WlSurface),
    X11(Box<X11Surface>),
}

impl KeyboardFocusTarget {
    pub fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            KeyboardFocusTarget::Wl(s) => Some(s.clone()),
            KeyboardFocusTarget::X11(s) => s.wl_surface(),
        }
    }
}

impl IsAlive for KeyboardFocusTarget {
    fn alive(&self) -> bool {
        match self {
            KeyboardFocusTarget::Wl(s) => s.alive(),
            KeyboardFocusTarget::X11(s) => s.alive(),
        }
    }
}

impl KeyboardTarget<State> for KeyboardFocusTarget {
    fn enter(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        keys: Vec<KeysymHandle<'_>>,
        serial: Serial,
    ) {
        match self {
            KeyboardFocusTarget::Wl(s) => KeyboardTarget::enter(s, seat, data, keys, serial),
            KeyboardFocusTarget::X11(s) => KeyboardTarget::enter(s.as_ref(), seat, data, keys, serial),
        }
    }

    fn leave(&self, seat: &Seat<State>, data: &mut State, serial: Serial) {
        match self {
            KeyboardFocusTarget::Wl(s) => KeyboardTarget::leave(s, seat, data, serial),
            KeyboardFocusTarget::X11(s) => KeyboardTarget::leave(s.as_ref(), seat, data, serial),
        }
    }

    fn key(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: InputTime,
    ) {
        match self {
            KeyboardFocusTarget::Wl(s) => {
                KeyboardTarget::key(s, seat, data, key, state, serial, time)
            }
            KeyboardFocusTarget::X11(s) => {
                KeyboardTarget::key(s.as_ref(), seat, data, key, state, serial, time)
            }
        }
    }

    fn modifiers(
        &self,
        seat: &Seat<State>,
        data: &mut State,
        modifiers: ModifiersState,
        serial: Serial,
    ) {
        match self {
            KeyboardFocusTarget::Wl(s) => {
                KeyboardTarget::modifiers(s, seat, data, modifiers, serial)
            }
            KeyboardFocusTarget::X11(s) => {
                KeyboardTarget::modifiers(s.as_ref(), seat, data, modifiers, serial)
            }
        }
    }
}

impl WaylandFocus for KeyboardFocusTarget {
    fn wl_surface(&self) -> Option<Cow<'_, WlSurface>> {
        match self {
            KeyboardFocusTarget::Wl(s) => Some(Cow::Borrowed(s)),
            KeyboardFocusTarget::X11(s) => s.wl_surface().map(Cow::Owned),
        }
    }
}

pub struct ClientState {
    pub compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, client_id: ClientId) {
        let _ = tunables();
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
    pub viewporter_state: ViewporterState,
    pub text_input_state: TextInputManagerState,
    pub xdg_decoration_state: XdgDecorationState,

    pub xwayland_shell_state: XWaylandShellState,
    pub xwms: HashMap<XwmId, X11Wm>,

    pub windows: Vec<ToplevelSurface>,
    pub x11_windows: Vec<X11Surface>,
    pub active_surface: Option<WlSurface>,
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
                        InputTime::from_millis(time),
                        |_, _, _| FilterResult::Forward,
                    );
                }
            }
            InputEvent::PointerMotion { x, y, time } => {
                let serial = SERIAL_COUNTER.next_serial();
                if let Some(pointer) = self.seat.get_pointer() {
                    let focus = self.main_surface().map(|s| (s, (0.0, 0.0).into()));

                    pointer.motion(
                        self,
                        focus,
                        &MotionEvent {
                            location: (x, y).into(),
                            serial,
                            time: InputTime::from_millis(time),
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
                if pressed
                    && let Some(target) = self.main_surface()
                    && self.focused_surface().as_ref() != Some(&target)
                {
                    self.focus_surface(Some(target));
                }
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
                            time: InputTime::from_millis(time),
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
                    let frame = AxisFrame::new(InputTime::from_millis(time))
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
                for window in &self.x11_windows {
                    let _ = window.close();
                }
            }
            InputEvent::Resize {
                height,
                width,
                time: _,
            } => {
                for window in &self.windows {
                    window.with_pending_state(|state| {
                        state.size = Some((width as i32, height as i32).into());
                    });

                    window.send_configure();
                }
                for window in self
                    .x11_windows
                    .iter()
                    .filter(|w| !w.is_override_redirect())
                    .filter(|w| tunables().focus == FocusMode::Legacy || Self::is_main_sized(w))
                {
                    let _ = window.configure(Rectangle::new(
                        Point::new(0, 0),
                        Size::new(width as i32, height as i32),
                    ));
                }
            }
        }
    }

    fn main_surface(&self) -> Option<WlSurface> {
        self.active_surface.clone()
    }

    fn root_surface(surface: &WlSurface) -> WlSurface {
        let mut current = surface.clone();
        while let Some(parent) = get_parent(&current) {
            current = parent;
        }
        current
    }

    fn focus_target_for(&self, surface: &WlSurface) -> KeyboardFocusTarget {
        if tunables().focus == FocusMode::Wl {
            return KeyboardFocusTarget::Wl(surface.clone());
        }
        self.x11_windows
            .iter()
            .find(|w| w.wl_surface().as_ref() == Some(surface))
            .map(|w| KeyboardFocusTarget::X11(Box::new(w.clone())))
            .unwrap_or_else(|| KeyboardFocusTarget::Wl(surface.clone()))
    }

    fn focused_surface(&self) -> Option<WlSurface> {
        self.seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .and_then(|f| f.wl_surface())
    }

    fn focus_surface(&mut self, surface: Option<WlSurface>) {
        if tunables().focus == FocusMode::Off && let Some(s) = surface.as_ref()
                && self
                    .x11_windows
                    .iter()
                    .any(|w| w.wl_surface().as_ref() == Some(s))
                {
                    return;
                }
        let target = surface.as_ref().map(|s| self.focus_target_for(s));
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, target, serial);
        }
    }

    fn focus_x11(&mut self, surface: &X11Surface) {
        match tunables().focus {
            FocusMode::Off => return,
            FocusMode::Wl => {
                if let Some(wl_surface) = surface.wl_surface() {
                    self.focus_surface(Some(wl_surface));
                }
                return;
            }
            FocusMode::Late | FocusMode::Legacy => {}
        }
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = SERIAL_COUNTER.next_serial();
            keyboard.set_focus(
                self,
                Some(KeyboardFocusTarget::X11(Box::new(surface.clone()))),
                serial,
            );
        }
    }

    fn is_target_x11(&self, surface: &X11Surface) -> bool {
        match crate::WINDOW.lock().unwrap().as_ref() {
            Some(target) => surface.title() == target.as_str(),
            None => true,
        }
    }

    fn is_main_sized(surface: &X11Surface) -> bool {
        let geo = surface.geometry();
        geo.size.w >= 320 && geo.size.h >= 200
    }

    fn should_become_active(&self, surface: &X11Surface) -> bool {
        if !self.is_target_x11(surface) {
            return false;
        }
        if !surface.is_override_redirect() {
            return tunables().focus == FocusMode::Legacy
                || Self::is_main_sized(surface)
                || self.active_surface.is_none();
        }
        let geo = surface.geometry();
        let (w, h) = (*WIDTH.lock().unwrap(), *HEIGHT.lock().unwrap());
        geo.size.w * 10 >= w * 9 && geo.size.h * 10 >= h * 9
    }

    fn try_activate(&mut self, surface: &X11Surface) {
        if !self
            .x11_windows
            .iter()
            .any(|w| w.window_id() == surface.window_id())
        {
            return;
        }
        if let Some(wl_surface) = surface.wl_surface() {
            if !self.should_become_active(surface) {
                info!(
                    "Ignoring X11 window {} title={:?} geometry={:?}",
                    surface.window_id(),
                    surface.title(),
                    surface.geometry()
                );
                return;
            }

            if self.active_surface.as_ref() != Some(&wl_surface) {
                info!(
                    "Display target is now X11 window {} title={:?} geometry={:?}",
                    surface.window_id(),
                    surface.title(),
                    surface.geometry()
                );
                self.active_surface = Some(wl_surface.clone());
            }

            if tunables().focus != FocusMode::Off
                && self.focused_surface().as_ref() != Some(&wl_surface)
            {
                info!(
                    "Focusing X11 window {} title={:?} mode={:?}",
                    surface.window_id(),
                    surface.title(),
                    tunables().focus
                );
                if tunables().focus == FocusMode::Late {
                    let _ = surface.set_activated(true);
                }
                self.focus_x11(surface);
            }
        };
    }

    fn maybe_report(&self) {
        let now = now_ms();
        let last = LAST_REPORT_MS.load(Ordering::Relaxed);
        if last == 0 {
            LAST_REPORT_MS.store(now, Ordering::Relaxed);
            return;
        }
        if now.saturating_sub(last) < 2000 {
            return;
        }
        LAST_REPORT_MS.store(now, Ordering::Relaxed);

        let commits = ACTIVE_COMMITS.swap(0, Ordering::Relaxed);
        let with_buffer = ACTIVE_BUFFERS.swap(0, Ordering::Relaxed);
        let captured = CAPTURED.swap(0, Ordering::Relaxed);

        let active_x11 = self.active_surface.as_ref().and_then(|a| {
            self.x11_windows
                .iter()
                .find(|w| w.wl_surface().as_ref() == Some(a))
        });
        match active_x11 {
            Some(w) => info!(
                "[stats] 2s: active_commits={} with_buffer={} captured={} | X11 {} {:?} {:?} fullscreen={} hidden={} activated={}",
                commits,
                with_buffer,
                captured,
                w.window_id(),
                w.title(),
                w.geometry(),
                w.is_fullscreen(),
                w.is_hidden(),
                w.is_activated()
            ),
            None => info!(
                "[stats] 2s: active_commits={} with_buffer={} captured={} | active={:?}",
                commits,
                with_buffer,
                captured,
                self.active_surface.as_ref().map(|s| s.id())
            ),
        }
    }

    fn forget_x11_window(&mut self, surface: &X11Surface) {
        self.x11_windows
            .retain(|w| w.window_id() != surface.window_id());

        let active_gone = match &self.active_surface {
            Some(active) => {
                !self.windows.iter().any(|w| w.wl_surface() == active)
                    && !self
                        .x11_windows
                        .iter()
                        .any(|w| w.wl_surface().as_ref() == Some(active))
            }
            None => false,
        };

        if active_gone {
            let next = self
                .x11_windows
                .iter()
                .rev()
                .find(|w| w.wl_surface().is_some() && self.should_become_active(w))
                .cloned();
            match next {
                Some(w) => {
                    self.active_surface = w.wl_surface();
                    self.focus_x11(&w);
                }
                None => self.active_surface = None,
            }
        }
    }
}

impl XwmHandler for State {
    fn xwm_state(&mut self, xwm: XwmId) -> &mut X11Wm {
        self.xwms.get_mut(&xwm).unwrap()
    }

    fn new_window(&mut self, id: XwmId, surface: X11Surface) {
        info!("New XWayland window: id: {:?}, surface: {:?}", id, surface);
    }

    fn map_window_request(&mut self, _id: XwmId, surface: X11Surface) {
        info!(
            "map_window_request: window={} title={:?} class={:?}",
            surface.window_id(),
            surface.title(),
            surface.class()
        );

        if let Err(e) = surface.set_mapped(true) {
            warn!("Failed to map X11 window {}: {:?}", surface.window_id(), e);
            return;
        }

        self.x11_windows.push(surface.clone());

        if tunables().focus == FocusMode::Legacy {
            if self.is_target_x11(&surface) {
                info!(
                    "Selecting X11 target: window={} title={:?}",
                    surface.window_id(),
                    surface.title()
                );
                let _ = surface.set_activated(true);
                if let Some(wl_surface) = surface.wl_surface() {
                    self.active_surface = Some(wl_surface);
                }
                self.focus_x11(&surface);
            }
            return;
        }

        self.try_activate(&surface);
    }

    fn unmapped_window(&mut self, _id: XwmId, surface: X11Surface) {
        self.forget_x11_window(&surface);
    }
    fn destroyed_window(&mut self, _id: XwmId, surface: X11Surface) {
        self.forget_x11_window(&surface);
    }

    fn disconnected(&mut self, xwm: XwmId) {
        self.xwms.remove(&xwm);
        self.x11_windows.clear();
    }

    fn new_override_redirect_window(&mut self, _id: XwmId, _surface: X11Surface) {}
    fn mapped_override_redirect_window(&mut self, _id: XwmId, surface: X11Surface) {
        info!(
            "override-redirect window mapped: window={} title={:?} class={:?} geometry={:?}",
            surface.window_id(),
            surface.title(),
            surface.class(),
            surface.geometry()
        );

        if !self
            .x11_windows
            .iter()
            .any(|w| w.window_id() == surface.window_id())
        {
            self.x11_windows.push(surface.clone());
        }

        if tunables().focus == FocusMode::Legacy {
            if self.should_become_active(&surface) {
                if let Some(wl_surface) = surface.wl_surface() {
                    self.active_surface = Some(wl_surface);
                }
                self.focus_x11(&surface);
            }
        } else {
            self.try_activate(&surface);
        }
    }

    fn configure_request(
        &mut self,
        _id: XwmId,
        surface: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        width: Option<u32>,
        height: Option<u32>,
        reorder: Option<Reorder>,
    ) {
        let geom = surface.geometry();

        let new_geom = Rectangle::new(
            Point::new(x.unwrap_or(geom.loc.x), y.unwrap_or(geom.loc.y)),
            Size::new(
                width.unwrap_or(geom.size.w.max(1) as u32) as i32,
                height.unwrap_or(geom.size.h.max(1) as u32) as i32,
            ),
        );

        if let Err(e) = surface.configure(new_geom) {
            warn!(
                "Failed to configure X11 window {}: {:?}",
                surface.window_id(),
                e
            );
        }

        if let Some(reorder) = reorder {
            match reorder {
                Reorder::Top => {
                    if let Some(pos) = self
                        .x11_windows
                        .iter()
                        .position(|w| w.window_id() == surface.window_id())
                    {
                        let window = self.x11_windows.remove(pos);
                        self.x11_windows.push(window);
                    }
                }

                Reorder::Above(sibling) => {
                    if let Some(pos) = self
                        .x11_windows
                        .iter()
                        .position(|w| w.window_id() == surface.window_id())
                    {
                        let window = self.x11_windows.remove(pos);

                        let sibling_pos = self
                            .x11_windows
                            .iter()
                            .position(|w| w.window_id() == sibling);

                        if let Some(pos) = sibling_pos {
                            self.x11_windows.insert(pos + 1, window);
                        } else {
                            self.x11_windows.push(window);
                        }
                    }
                }

                Reorder::Below(sibling) => {
                    if let Some(pos) = self
                        .x11_windows
                        .iter()
                        .position(|w| w.window_id() == surface.window_id())
                    {
                        let window = self.x11_windows.remove(pos);

                        let sibling_pos = self
                            .x11_windows
                            .iter()
                            .position(|w| w.window_id() == sibling);

                        if let Some(pos) = sibling_pos {
                            self.x11_windows.insert(pos, window);
                        } else {
                            self.x11_windows.insert(0, window);
                        }
                    }
                }

                Reorder::Bottom => {
                    if let Some(pos) = self
                        .x11_windows
                        .iter()
                        .position(|w| w.window_id() == surface.window_id())
                    {
                        let window = self.x11_windows.remove(pos);
                        self.x11_windows.insert(0, window);
                    }
                }
            }
        }
    }

    fn configure_notify(
        &mut self,
        _id: XwmId,
        _surface: X11Surface,
        _geometry: Rectangle<i32, Logical>,
        _above: Option<u32>,
    ) {
    }

    fn resize_request(
        &mut self,
        _id: XwmId,
        _surface: X11Surface,
        _button: u32,
        _edges: ResizeEdge,
    ) {
    }
    fn move_request(&mut self, _id: XwmId, _surface: X11Surface, _button: u32) {}

    fn minimize_request(&mut self, _id: XwmId, window: X11Surface) {
        info!(
            "minimize_request for {} ({:?}) - ignoring",
            window.window_id(),
            window.title()
        );
    }

    fn unminimize_request(&mut self, _id: XwmId, window: X11Surface) {
        info!(
            "unminimize_request for {} ({:?})",
            window.window_id(),
            window.title()
        );
    }

    fn fullscreen_request(&mut self, _id: XwmId, window: X11Surface) {
        let (w, h) = (*WIDTH.lock().unwrap(), *HEIGHT.lock().unwrap());
        info!(
            "fullscreen_request for {} ({:?}) -> {}x{}",
            window.window_id(),
            window.title(),
            w,
            h
        );
        let _ = window.set_fullscreen(true);
        let _ = window.configure(Rectangle::new(Point::new(0, 0), Size::new(w, h)));
    }

    fn unfullscreen_request(&mut self, _id: XwmId, window: X11Surface) {
        info!(
            "unfullscreen_request for {} ({:?})",
            window.window_id(),
            window.title()
        );
        let _ = window.set_fullscreen(false);
    }
}

impl CompositorHandler for State {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        if let Some(data) = client.get_data::<XWaylandClientData>() {
            return &data.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        let root = Self::root_surface(surface);

        let is_xdg_toplevel = self.windows.iter().any(|w| *w.wl_surface() == root);
        let is_x11_window = self
            .x11_windows
            .iter()
            .any(|w| w.wl_surface().as_ref() == Some(&root));

        if !is_xdg_toplevel && !is_x11_window {
            return;
        }

        let is_active = self.active_surface.as_ref() == Some(&root);

        let capture = match crate::WINDOW.lock().unwrap().as_ref() {
            Some(window_name) => self.x11_windows.iter().any(|window| {
                window.wl_surface().as_ref() == Some(&root) && &window.title() == window_name
            }),
            None => match tunables().capture {
                CaptureMode::Any => true,
                CaptureMode::Auto => is_active,
            },
        };

        let buffer = with_states(surface, |states| {
            let mut attributes = states.cached_state.get::<SurfaceAttributes>();
            let attributes = attributes.current();

            match &attributes.buffer {
                Some(BufferAssignment::NewBuffer(buffer)) => Some(buffer.clone()),
                _ => None,
            }
        });

        if is_active {
            ACTIVE_COMMITS.fetch_add(1, Ordering::Relaxed);
            if buffer.is_some() {
                ACTIVE_BUFFERS.fetch_add(1, Ordering::Relaxed);
            }
            trace!(
                "active commit surface={:?} has_buffer={} capture={}",
                surface.id(),
                buffer.is_some(),
                capture
            );
        }

        if let Some(buffer) = buffer.filter(|_| capture) {
            match with_buffer_contents(&buffer, |ptr, len, metadata| {
                let (width, height, stride) = (metadata.width, metadata.height, metadata.stride);

                let mut frame = self.shared_frame.lock().unwrap();

                frame.width = width as u32;
                frame.height = height as u32;
                frame.pixels.resize((width * height) as usize, 0);

                let src = unsafe { std::slice::from_raw_parts(ptr, len) };

                for y in 0..height as usize {
                    let row_start = y * stride as usize;
                    let row_end = row_start + width as usize * 4;

                    if row_end > src.len() {
                        warn!(
                            "bad row {} (stride={} fmt={:?} len={})",
                            y,
                            stride,
                            metadata.format,
                            src.len()
                        );
                        continue;
                    }

                    let row = &src[row_start..row_end];

                    for x in 0..width as usize {
                        let b = row[x * 4] as u32;
                        let g = row[x * 4 + 1] as u32;
                        let r = row[x * 4 + 2] as u32;

                        frame.pixels[y * width as usize + x] = (r << 16) | (g << 8) | b;
                    }
                }
            }) {
                Ok(()) => {
                    CAPTURED.fetch_add(1, Ordering::Relaxed);
                    buffer.release();
                    self.proxy
                        .send_event(UserEvent::FrameReady(self.cursor.clone()))
                        .ok();
                }

                Err(e) => {
                    warn!("Could not read XWayland buffer as SHM: {:?}", e);
                }
            }
        }

        self.maybe_report();

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

impl XdgShellHandler for State {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let wl_surface = surface.wl_surface().clone();

        info!("New toplevel: {:?}", wl_surface.id());

        surface.send_configure();

        self.windows.push(surface);

        self.active_surface = Some(wl_surface.clone());

        self.focus_surface(Some(wl_surface));
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.windows
            .retain(|w| w.wl_surface() != surface.wl_surface());
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

    fn fullscreen_request(&mut self, _surface: ToplevelSurface, _output: Option<WlOutput>) {
        self.proxy
            .send_event(UserEvent::FullscreenRequest(true))
            .ok();
    }

    fn unfullscreen_request(&mut self, _surface: ToplevelSurface) {
        self.proxy
            .send_event(UserEvent::FullscreenRequest(false))
            .ok();
    }
}

impl SeatHandler for State {
    type KeyboardFocus = KeyboardFocusTarget;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        self.cursor = image;
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, _focused: Option<&KeyboardFocusTarget>) {}
}

impl ShmHandler for State {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl BufferHandler for State {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

impl DataDeviceHandler for State {
    fn data_device_state(&mut self) -> &mut DataDeviceState {
        &mut self.data_device_state
    }
}

impl SelectionHandler for State {
    type SelectionUserData = ();
}

impl WaylandDndGrabHandler for State {}
impl DndGrabHandler for State {}

impl OutputHandler for State {}

impl XdgDecorationHandler for State {
    fn new_decoration(&mut self, _toplevel: ToplevelSurface) {}

    fn request_mode(&mut self, _toplevel: ToplevelSurface, _mode: Mode) {}

    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
}

impl XWaylandShellHandler for State {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    fn surface_associated(&mut self, _xwm: XwmId, wl_surface: WlSurface, surface: X11Surface) {
        info!(
            "X11 window {} associated with {:?}, title={:?}",
            surface.window_id(),
            wl_surface.id(),
            surface.title()
        );

        let mapped = self
            .x11_windows
            .iter()
            .any(|w| w.window_id() == surface.window_id());

        if !mapped {
            info!(
                "window {} associated but not mapped yet",
                surface.window_id()
            );
            return;
        }

        if tunables().focus == FocusMode::Legacy {
            if !self.should_become_active(&surface) {
                info!(
                    "Ignoring non-target X11 window {} title={:?}",
                    surface.window_id(),
                    surface.title()
                );
                return;
            }

            self.active_surface = Some(wl_surface.clone());
            self.focus_surface(Some(wl_surface));
            return;
        }

        self.try_activate(&surface);
    }
}

impl PointerConstraintsHandler for State {}
impl TextInputActivation for State {}

delegate_dispatch2!(State);
