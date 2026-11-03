use crate::{SAVE, input::InputEvent};
use shared_memory::ShmemConf;
use std::{
    sync::{
        OnceLock,
        mpsc::{self, Sender},
    },
    time::{Duration, Instant},
};
use tracing::{error, info};

pub struct PendingFrame {
    pub pixels: Vec<u32>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy)]
pub enum TrainerSignal {
    Reward,
    Loss,
}

pub enum IpcMessage {
    Frame(PendingFrame),
    Trainer(TrainerSignal),
}

#[repr(C, align(8))]
struct SharedControlBlock {
    python_response_byte: u8,
    _padding: [u8; 7],
    rust_frame_counter: i64,
    python_frame_ack: i64,
    trainer_seq: i64,
    trainer_value: i64,
}

pub static IPC_TX: OnceLock<Sender<IpcMessage>> = OnceLock::new();

pub fn send_trainer_signal(signal: TrainerSignal) {
    if let Some(tx) = IPC_TX.get() {
        let _ = tx.send(IpcMessage::Trainer(signal));
    }
}

pub fn init_ipc_worker(input_tx: Sender<InputEvent>) {
    let (tx, rx) = mpsc::channel::<IpcMessage>();
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
            let mut trainer_seq: i64 = 0;

            let save = SAVE.lock().unwrap();
            let lane_1_size = (save.options.width * save.options.height * 3) as usize;
            let control_block_offset = (lane_1_size + 7) & !7;

            if control_block_offset + std::mem::size_of::<SharedControlBlock>() > shmem.len() {
                error!("❌ Shared memory segment too small for control block");
                std::process::exit(1);
            }

            let control_ptr =
                unsafe { base_raw_ptr.add(control_block_offset) as *mut SharedControlBlock };

            println!("Asynchronous IPC Worker initialized successfully!");

            while let Ok(msg) = rx.recv() {
                match msg {
                    IpcMessage::Frame(frame) => unsafe {
                        if lane_1_size > shmem.len() {
                            info!(
                                "Frame size ({} bytes) exceeds shared memory pool limit",
                                lane_1_size
                            );
                            continue;
                        }

                        let rust_to_py_tx_slice =
                            std::slice::from_raw_parts_mut(base_raw_ptr, lane_1_size);

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
                        std::ptr::write_volatile(
                            std::ptr::addr_of_mut!((*control_ptr).rust_frame_counter),
                            local_counter,
                        );

                        let spin_start = Instant::now();
                        let python_ack_ptr = std::ptr::addr_of!((*control_ptr).python_frame_ack);
                        while std::ptr::read_volatile(python_ack_ptr) < local_counter {
                            if spin_start.elapsed() > Duration::from_millis(16) {
                                break;
                            }
                            std::thread::yield_now();
                        }

                        let command_byte = std::ptr::read_volatile(std::ptr::addr_of!(
                            (*control_ptr).python_response_byte
                        ));

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
                    },

                    IpcMessage::Trainer(signal) => unsafe {
                        trainer_seq += 1;
                        let value: i64 = match signal {
                            TrainerSignal::Reward => 1,
                            TrainerSignal::Loss => -1,
                        };

                        std::ptr::write_volatile(
                            std::ptr::addr_of_mut!((*control_ptr).trainer_value),
                            value,
                        );
                        std::ptr::write_volatile(
                            std::ptr::addr_of_mut!((*control_ptr).trainer_seq),
                            trainer_seq,
                        );

                        info!("Sent trainer signal: {:?}", signal);
                    },
                }
            }
        })
        .unwrap();
}
