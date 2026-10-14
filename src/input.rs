use winit::{event::MouseButton, keyboard::KeyCode};

#[derive(Debug, Clone, Copy)]
pub enum InputEvent {
    Keyboard {
        keycode: u32,
        pressed: bool,
        time: u32,
    },
    PointerMotion {
        x: f64,
        y: f64,
        time: u32,
    },
    PointerButton {
        button: u32,
        pressed: bool,
        time: u32,
    },
    PointerAxis {
        horizontal: f64,
        vertical: f64,
        time: u32,
    },
    Resize {
        height: u32,
        width: u32,
    },
    CloseRequest,
}

pub fn linux_mouse_button(button: MouseButton) -> Option<u32> {
    match button {
        MouseButton::Left => Some(0x110),
        MouseButton::Right => Some(0x111),
        MouseButton::Middle => Some(0x112),
        _ => None,
    }
}

pub fn linux_keycode(code: KeyCode) -> Option<u32> {
    use winit::keyboard::KeyCode::*;
    Some(match code {
        KeyA => 30,
        KeyB => 48,
        KeyC => 46,
        KeyD => 32,
        KeyE => 18,
        KeyF => 33,
        KeyG => 34,
        KeyH => 35,
        KeyI => 23,
        KeyJ => 36,
        KeyK => 37,
        KeyL => 38,
        KeyM => 50,
        KeyN => 49,
        KeyO => 24,
        KeyP => 25,
        KeyQ => 16,
        KeyR => 19,
        KeyS => 31,
        KeyT => 20,
        KeyU => 22,
        KeyV => 47,
        KeyW => 17,
        KeyX => 45,
        KeyY => 21,
        KeyZ => 44,

        Digit0 => 11,
        Digit1 => 2,
        Digit2 => 3,
        Digit3 => 4,
        Digit4 => 5,
        Digit5 => 6,
        Digit6 => 7,
        Digit7 => 8,
        Digit8 => 9,
        Digit9 => 10,

        Escape => 1,
        Enter => 28,
        Space => 57,
        Tab => 15,
        Backspace => 14,

        ShiftLeft => 42,
        ShiftRight => 54,
        ControlLeft => 29,
        ControlRight => 97,
        AltLeft => 56,
        AltRight => 100,

        ArrowUp => 103,
        ArrowLeft => 105,
        ArrowRight => 106,
        ArrowDown => 108,
        Home => 102,
        End => 107,
        PageUp => 104,
        PageDown => 109,
        Insert => 110,
        Delete => 111,

        F1 => 59,
        F2 => 60,
        F3 => 61,
        F4 => 62,
        F5 => 63,
        F6 => 64,
        F7 => 65,
        F8 => 66,
        F9 => 67,
        F10 => 68,
        F11 => 87,
        F12 => 88,

        _ => return None,
    })
}
