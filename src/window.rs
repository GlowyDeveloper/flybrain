use crate::{
    SAVE,
    framebuffer::SharedFrame,
    input::{InputEvent, linux_keycode, linux_mouse_button},
    memory_bridge::{TrainerSignal, send_trainer_signal},
};
use smithay::input::pointer::CursorImageStatus;
use softbuffer::{Context, Surface};
use std::{num::NonZeroU32, rc::Rc, sync::mpsc::Sender, time::Instant};
use tracing::{debug, trace};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, Size},
    event::{ElementState, MouseScrollDelta, WindowEvent},
    event_loop::ActiveEventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::{CursorIcon, Fullscreen, Window, WindowAttributes, WindowId},
};

pub enum UserEvent {
    FrameReady(CursorImageStatus),
    FullscreenRequest(bool),
}

pub struct App {
    window: Option<Rc<Window>>,
    surface: Option<Surface<Rc<Window>, Rc<Window>>>,
    shared: SharedFrame,
    input_tx: Sender<InputEvent>,
    start: Instant,
}

impl App {
    pub fn new(shared: SharedFrame, input_tx: Sender<InputEvent>) -> Self {
        App {
            window: None,
            surface: None,
            shared,
            input_tx,
            start: Instant::now(),
        }
    }

    fn now(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn scale(&self) -> f64 {
        self.window
            .as_ref()
            .map(|w| w.scale_factor())
            .unwrap_or(1.0)
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let save = SAVE.lock().unwrap();
        let size = Size::Logical(LogicalSize::new(
            save.options.width as f64,
            save.options.height as f64,
        ));
        let mut attr = WindowAttributes::default()
            .with_decorations(false)
            .with_resizable(true)
            .with_inner_size(size)
            .with_min_inner_size(size);

        if save.options.fullscreen {
            attr = attr.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }

        let window = Rc::new(event_loop.create_window(attr).unwrap());

        let context = Context::new(window.clone()).unwrap();
        let surface = Surface::new(&context, window.clone()).unwrap();
        self.window = Some(window.clone());
        self.surface = Some(surface);
        window.request_redraw();
    }

    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::RedrawRequested => {
                let frame = self.shared.lock().unwrap();

                if frame.width == 0 || frame.height == 0 {
                    return;
                }

                let surface = self.surface.as_mut().unwrap();

                surface
                    .resize(
                        NonZeroU32::new(frame.width.max(1)).unwrap(),
                        NonZeroU32::new(frame.height.max(1)).unwrap(),
                    )
                    .unwrap();
                let mut buf = surface.buffer_mut().unwrap();
                buf.copy_from_slice(&frame.pixels);
                buf.present().unwrap();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.repeat {
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    if event.state == ElementState::Pressed {
                        match code {
                            KeyCode::F9 => {
                                send_trainer_signal(TrainerSignal::Reward);
                                return;
                            }
                            KeyCode::F10 => {
                                send_trainer_signal(TrainerSignal::Loss);
                                return;
                            }
                            _ => {}
                        }
                    }
                    if let Some(keycode) = linux_keycode(code) {
                        let _ = self.input_tx.send(InputEvent::Keyboard {
                            keycode,
                            pressed: event.state == ElementState::Pressed,
                            time: self.now(),
                        });
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = position.to_logical::<f64>(self.scale());
                let _ = self.input_tx.send(InputEvent::PointerMotion {
                    x: p.x,
                    y: p.y,
                    time: self.now(),
                });
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(code) = linux_mouse_button(button) {
                    let _ = self.input_tx.send(InputEvent::PointerButton {
                        button: code,
                        pressed: state == ElementState::Pressed,
                        time: self.now(),
                    });
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (h, v) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (x as f64 * 15.0, y as f64 * 15.0),
                    MouseScrollDelta::PixelDelta(p) => (p.x, p.y),
                };
                let _ = self.input_tx.send(InputEvent::PointerAxis {
                    horizontal: h,
                    vertical: -v,
                    time: self.now(),
                });
            }
            WindowEvent::CloseRequested => {
                trace!("Close requested");
                let _ = self.input_tx.send(InputEvent::CloseRequest);
            }
            WindowEvent::Resized(resize) => {
                let logical = resize.to_logical::<f64>(self.scale());
                let (h, w) = (logical.height.round() as u32, logical.width.round() as u32);
                if h == 0 || w == 0 {
                    return;
                }
                let _ = self.input_tx.send(InputEvent::Resize {
                    height: h,
                    width: w,
                    time: self.now(),
                });
            }
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        if let Some(window) = &self.window {
            let save = SAVE.lock().unwrap();
            match event {
                UserEvent::FullscreenRequest(fullscreen) if !save.options.fullscreen => {
                    window.set_fullscreen(if fullscreen {
                        Some(Fullscreen::Borderless(None))
                    } else {
                        None
                    });
                }
                UserEvent::FrameReady(cursor) => {
                    match cursor {
                        CursorImageStatus::Named(icon_name) => {
                            window.set_cursor_visible(true);

                            debug!("Icon: {}", icon_name.name());

                            let winit_icon = match icon_name.name() {
                                "default" | "left_ptr" => CursorIcon::Default,
                                "pointer" | "hand2" => CursorIcon::Pointer,
                                "text" | "xterm" => CursorIcon::Text,
                                "grab" => CursorIcon::Grab,
                                "grabbing" => CursorIcon::Grabbing,
                                "help" => CursorIcon::Help,
                                "progress" => CursorIcon::Progress,
                                "wait" => CursorIcon::Wait,
                                "crosshair" => CursorIcon::Crosshair,
                                "not-allowed" => CursorIcon::NotAllowed,
                                "n-resize" => CursorIcon::NResize,
                                "s-resize" => CursorIcon::SResize,
                                "e-resize" => CursorIcon::EResize,
                                "w-resize" => CursorIcon::WResize,
                                _ => CursorIcon::Default,
                            };
                            window.set_cursor(winit_icon);
                        }
                        CursorImageStatus::Hidden => {
                            debug!("Hiding icon");
                            window.set_cursor_visible(false);
                        }
                        CursorImageStatus::Surface(_) => {}
                    }

                    trace!("Redraw requested");
                    window.request_redraw();
                }
                _ => {}
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {}
}
