use crate::{ENV, SAVE, input::InputEvent};
use shared_memory::ShmemConf;
use std::{
    sync::{
        Arc, Mutex, OnceLock,
        mpsc::{self, Sender},
    },
    time::{Duration, Instant},
};
use tracing::{error, info, warn};

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
struct SharedControlTail {
    rust_frame_counter: i64,
    python_frame_ack: i64,
    trainer_seq: i64,
    trainer_value: i64,
}

struct ActionChannel {
    keycode: u32,
    pressed_byte: u8,
    released_byte: u8,
}

pub struct FrameSlot {
    pub latest: Mutex<Option<PendingFrame>>,
    pub notification: Sender<()>,
}

pub static IPC_TX: OnceLock<Sender<IpcMessage>> = OnceLock::new();

pub static FRAME_SLOT: OnceLock<Arc<FrameSlot>> = OnceLock::new();

pub fn send_trainer_signal(signal: TrainerSignal) {
    if let Some(tx) = IPC_TX.get() {
        let _ = tx.send(IpcMessage::Trainer(signal));
    }
}

pub fn init_ipc_worker(input_tx: Sender<InputEvent>) {
    let (tx, rx) = mpsc::channel::<IpcMessage>();
    let (frame_tx, frame_rx) = mpsc::channel::<()>();

    let _ = IPC_TX.set(tx);

    let frame_slot = Arc::new(FrameSlot {
        latest: Mutex::new(None),
        notification: frame_tx,
    });

    let _ = FRAME_SLOT.set(frame_slot.clone());

    std::thread::Builder::new()
        .name("flybrain_ipc_worker".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_millis(600));

            let shmem = match ShmemConf::new().os_id("flybrain_ipc_buffer").open() {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to attach to shared memory buffer: {}", e);
                    std::process::exit(1);
                }
            };

            let base_raw_ptr = shmem.as_ptr();
            let mut local_counter: i64 = 0;
            let mut trainer_seq: i64 = 0;

            let (lane_1_size, actions) = {
                let save = SAVE.lock().unwrap();
                let env = ENV.lock().unwrap();

                let lane_1_size = if env.shared.lane_1 == "default" {
                    (save.options.width * save.options.height * 3) as usize
                } else if let Ok(lane_1) = env.shared.lane_1.parse::<usize>() {
                    lane_1
                } else {
                    warn!("Unable to parse lane_1 size to usize");
                    (save.options.width * save.options.height * 3) as usize
                };

                let mut action_codes: Vec<u32> = save.inputs.keys().copied().collect();
                action_codes.sort();
                let actions: Vec<ActionChannel> = action_codes
                    .iter()
                    .map(|code| {
                        let input = &save.inputs[code];
                        ActionChannel {
                            keycode: *code,
                            pressed_byte: input.pressed,
                            released_byte: input.released,
                        }
                    })
                    .collect();

                (lane_1_size, actions)
            };

            let n_actions = actions.len();

            let response_bytes_offset = lane_1_size;
            let control_block_offset = (response_bytes_offset + n_actions + 7) & !7;

            if control_block_offset + std::mem::size_of::<SharedControlTail>() > shmem.len() {
                error!("❌ Shared memory segment too small for control block");
                std::process::exit(1);
            }

            let response_bytes_ptr =
                unsafe { base_raw_ptr.add(response_bytes_offset) as *const u8 };
            let control_ptr =
                unsafe { base_raw_ptr.add(control_block_offset) as *mut SharedControlTail };

            println!("Asynchronous IPC Worker initialized successfully!");

            while let Ok(()) | Err(mpsc::RecvTimeoutError::Timeout) =
                frame_rx.recv_timeout(Duration::from_millis(2))
            {
                let frame = frame_slot.latest.lock().unwrap().take();

                if let Some(frame) = frame {
                    unsafe {
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

                        let timestamp = spin_start.elapsed().as_millis() as u32;
                        for (i, action) in actions.iter().enumerate() {
                            let command_byte = std::ptr::read_volatile(response_bytes_ptr.add(i));

                            if command_byte == action.pressed_byte {
                                let _ = input_tx.send(InputEvent::Keyboard {
                                    keycode: action.keycode,
                                    pressed: true,
                                    time: timestamp,
                                });
                            } else if command_byte == action.released_byte {
                                let _ = input_tx.send(InputEvent::Keyboard {
                                    keycode: action.keycode,
                                    pressed: false,
                                    time: timestamp + 20,
                                });
                            }
                        }
                    }
                }

                while let Ok(IpcMessage::Trainer(signal)) = rx.try_recv() {
                    unsafe {
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
                    }
                }
            }
        })
        .unwrap();
}
