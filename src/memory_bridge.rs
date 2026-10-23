use crate::input::InputEvent;
use shared_memory::ShmemConf;
use std::{time::{Duration, Instant}, sync::{mpsc::{self, Sender}, OnceLock}};
use tracing::{error, info};

pub struct PendingFrame {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

#[repr(C, align(8))]
struct SharedControlBlock {
    python_response_byte: u8,
    _padding: [u8; 7],
    rust_frame_counter: i64,
    python_frame_ack: i64,
}

pub static IPC_TX: OnceLock<Sender<PendingFrame>> = OnceLock::new();

pub fn init_ipc_worker(input_tx: Sender<InputEvent>) {
    let (tx, rx) = mpsc::channel::<PendingFrame>();
    let _ = IPC_TX.set(tx);

    std::thread::Builder::new()
        .name("flybrain_ipc_worker".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(600));

            let shmem = match ShmemConf::new().os_id("flybrain_ipc_buffer").open() {
                Ok(s) => s,
                Err(e) => {
                    error!("❌ Failed to attach to shared memory buffer: {}", e);
                    std::process::exit(1);
                }
            };

            let base_raw_ptr = shmem.as_ptr();
            let mut local_counter: i64 = 0;

            println!("Asynchronous IPC Worker initialized successfully!");

            while let Ok(frame) = rx.recv() {
                let lane_1_size = (frame.width * frame.height * 3) as usize;

                unsafe {
                    if lane_1_size > shmem.len() {
                        info!(
                            "Frame size ({} bytes) exceeds shared memory allocation pool limit by {} bytes",
                            lane_1_size, lane_1_size - shmem.len()
                        );
                        continue;
                    }

                    let rust_to_py_tx_slice =
                        std::slice::from_raw_parts_mut(base_raw_ptr, lane_1_size);

                    let control_block_offset = (lane_1_size + 7) & !7;

                    if control_block_offset + std::mem::size_of::<SharedControlBlock>()
                        > shmem.len()
                    {
                        continue;
                    }

                    let control_ptr =
                        base_raw_ptr.add(control_block_offset) as *mut SharedControlBlock;

                    let mut idx = 0;
                    for pixel in frame.pixels {
                        if idx + 2 < lane_1_size {
                            rust_to_py_tx_slice[idx] = ((pixel >> 16) & 0xFF) as u8;
                            rust_to_py_tx_slice[idx + 1] = ((pixel >> 8) & 0xFF) as u8;
                            rust_to_py_tx_slice[idx + 2] = (pixel & 0xFF) as u8;
                            idx += 3;
                        } else {
                            break;
                        }
                    }

                    local_counter += 1;
                    let rust_counter_ptr =
                        std::ptr::addr_of_mut!((*control_ptr).rust_frame_counter);
                    std::ptr::write_volatile(rust_counter_ptr, local_counter);

                    let spin_start = Instant::now();
                    let python_ack_ptr = std::ptr::addr_of!((*control_ptr).python_frame_ack);

                    while std::ptr::read_volatile(python_ack_ptr) < local_counter {
                        if spin_start.elapsed() > Duration::from_millis(16) {
                            break;
                        }
                        std::thread::yield_now();
                    }

                    let response_byte_ptr = std::ptr::addr_of!((*control_ptr).python_response_byte);
                    let command_byte = std::ptr::read_volatile(response_byte_ptr);

                    if command_byte == 0x20 {
                        let timestamp = spin_start.elapsed().as_millis() as u32;

                        let _ = input_tx.send(InputEvent::Keyboard {
                            keycode: 57,
                            pressed: true,
                            time: timestamp,
                        });
                    }

                    if command_byte == 0x21 {
                        let timestamp = spin_start.elapsed().as_millis() as u32;

                        let _ = input_tx.send(InputEvent::Keyboard {
                            keycode: 57,
                            pressed: false,
                            time: timestamp + 20,
                        });
                    }
                }
            }
        })
        .unwrap();
}
