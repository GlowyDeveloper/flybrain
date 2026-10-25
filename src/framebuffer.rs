use std::sync::{Arc, Mutex};

pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Default for FrameBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameBuffer {
    pub fn new() -> Self {
        Self {
            width: 0,
            height: 0,
            pixels: Vec::new(),
        }
    }
}

pub type SharedFrame = Arc<Mutex<FrameBuffer>>;
