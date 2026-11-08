import sys
import time
import pathlib
import tomllib
from multiprocessing import shared_memory
import numpy as np
from flybrain import FlyBrain

SHM_NAME = "flybrain_ipc_buffer"
FLYBRAIN_FOLDER = pathlib.Path(sys.argv[1])

with open(FLYBRAIN_FOLDER / "current.toml", "rb") as f:
    current = tomllib.load(f)

with open(FLYBRAIN_FOLDER / "env.toml", "rb") as f:
    env = tomllib.load(f)

lane_1 = env["shared"]["lane_1"]
if lane_1 == "default":
    lane_1 = int(current["options"]["width"]) * int(current["options"]["height"]) * 3
else:
    lane_1 = int(lane_1)

lane_2 = int(env["shared"]["lane_2"])
width = int(current["options"]["width"])
height = int(current["options"]["height"])
control_size = 64
total = lane_1 + lane_2 + control_size
brain_dir = FLYBRAIN_FOLDER / "brain"

try:
    zombie = shared_memory.SharedMemory(name=SHM_NAME)
    zombie.close()
    zombie.unlink()
except FileNotFoundError:
    pass

shm = shared_memory.SharedMemory(name=SHM_NAME, create=True, size=total)
py_to_rust_buf = shm.buf[0:lane_1]
control_block_offset = (lane_1 + 7) & ~7
rust_to_py_buf = shm.buf[control_block_offset : control_block_offset + 1]
control_flags  = shm.buf[control_block_offset + 8  : control_block_offset + 24]
trainer_flags  = shm.buf[control_block_offset + 24 : control_block_offset + 40]

rx_frame = np.ndarray((height, width, 3), dtype=np.uint8, buffer=py_to_rust_buf)
tx_byte = np.ndarray((1,), dtype=np.uint8, buffer=rust_to_py_buf)
sync_markers = np.ndarray((2,), dtype=np.int64, buffer=control_flags)
trainer_markers = np.ndarray((2,), dtype=np.int64, buffer=trainer_flags)

brain = FlyBrain(data=brain_dir, device="auto")

loom_detectors = brain.cells(["LC4", "LPLC2"])
escape_neurons = brain.cells(["DNp01"])
TRACKED = list(loom_detectors) + list(escape_neurons)
TRACKED_INDEX = {nid: i for i, nid in enumerate(TRACKED)}
N_FEATURES = len(TRACKED) + 1

JUMP_PRESS, JUMP_RELEASE = 0x20, 0x21

WEIGHTS_PATH = pathlib.Path(brain_dir).parent / "current.npz"
LEARNING_RATE = 0.05
TRACE_DECAY = 0.9

if WEIGHTS_PATH.exists():
    saved = np.load(WEIGHTS_PATH)
    w, b = saved["w"], float(saved["b"])
    print("Loaded trained policy")
else:
    w, b = np.zeros(N_FEATURES, dtype=np.float64), 0.0

e_w = np.zeros_like(w)
e_b = 0.0
rng = np.random.default_rng()

def sigmoid(x):
    return 1.0 / (1.0 + np.exp(-x))

def features_from(fired, delta):
    x = np.zeros(N_FEATURES, dtype=np.float64)
    for nid in fired:
        idx = TRACKED_INDEX.get(nid)
        if idx is not None:
            x[idx] = 1.0
    x[-1] = delta
    return x

prev_strip = None
jumping = False
last_trainer_seq = 0

try:
    print("Pipeline active. Awaiting compositor frames...")
    print("F9 = reward the last few decisions, F10 = penalize them.")
    last_seen_frame = 0

    while True:
        current_rust_frame = sync_markers[0]

        if current_rust_frame > last_seen_frame:
            last_seen_frame = current_rust_frame

            strip = rx_frame[0:height, 0:width].astype(np.float32)
            delta = 0.0 if prev_strip is None else np.abs(strip - prev_strip).mean() / 255.0
            prev_strip = strip

            fired = brain.step(inject=[(loom_detectors, float(delta))])
            x = features_from(fired, delta)

            p = sigmoid(w @ x + b)
            should_jump = bool(rng.random() < p)

            e_w = TRACE_DECAY * e_w + x * (float(should_jump) - p)
            e_b = TRACE_DECAY * e_b + (float(should_jump) - p)

            if should_jump and not jumping:
                tx_byte[0] = JUMP_PRESS
                jumping = True
            elif jumping:
                tx_byte[0] = JUMP_RELEASE
                jumping = False

            sync_markers[1] = last_seen_frame

        trainer_seq = int(trainer_markers[0])
        if trainer_seq != last_trainer_seq:
            last_trainer_seq = trainer_seq
            reward = float(trainer_markers[1])

            w += LEARNING_RATE * reward * e_w
            b += LEARNING_RATE * reward * e_b
            e_w[:] = 0.0
            e_b = 0.0

            np.savez(WEIGHTS_PATH, w=w, b=b)
            print(f"{'Rewarded' if reward > 0 else 'Penalized'} — policy saved")

        time.sleep(0.001)
except KeyboardInterrupt:
    pass
finally:
    np.savez(WEIGHTS_PATH, w=w, b=b)
    del rx_frame, tx_byte, sync_markers, trainer_markers
    del py_to_rust_buf, rust_to_py_buf, control_flags, trainer_flags
    shm.close()
    shm.unlink()
    print("\nShared memory unlinked cleanly.")