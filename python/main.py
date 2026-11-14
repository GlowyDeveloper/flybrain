import sys
import time
import pathlib
import tomllib
from multiprocessing import shared_memory
from zipfile import BadZipfile
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

lane_2 = env["shared"]["lane_2"]
if lane_2 == "default":
    lane_2 = len(current["inputs"])
else:
    lane_2 = int(lane_2)

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
lane_2_buf = shm.buf[lane_1 : lane_1 + lane_2]
control_block_offset = (lane_1 + lane_2 + 7) & ~7
control_flags  = shm.buf[control_block_offset      : control_block_offset + 16]
trainer_flags  = shm.buf[control_block_offset + 16 : control_block_offset + 32]

rx_frame = np.ndarray((height, width, 3), dtype=np.uint8, buffer=py_to_rust_buf)
sync_markers = np.ndarray((2,), dtype=np.int64, buffer=control_flags)
trainer_markers = np.ndarray((2,), dtype=np.int64, buffer=trainer_flags)

brain = FlyBrain(data=brain_dir, device="auto")

action_codes = sorted(current["inputs"].keys(), key=int)
actions = [
    {
        "code": code,
        "neuron": current["inputs"][code]["neuron"],
        "pressed": int(current["inputs"][code]["pressed"]),
        "released": int(current["inputs"][code]["released"]),
    }
    for code in action_codes
]
n_actions = len(actions)

if lane_2 != n_actions:
    raise ValueError(
        f"lane_2 size ({lane_2}) doesn't match the number of configured "
        f"inputs ({n_actions}); check env.toml's [shared] lane_2 setting."
    )

tx_bytes = np.ndarray((n_actions,), dtype=np.uint8, buffer=lane_2_buf)

loom_detectors = brain.cells(["LC4", "LPLC2"])
action_neurons = brain.cells([a["neuron"] for a in actions])

TRACKED = list(loom_detectors) + list(action_neurons)
TRACKED_INDEX = {nid: i for i, nid in enumerate(TRACKED)}
N_FEATURES = len(TRACKED) + 1

WEIGHTS_PATH = pathlib.Path(brain_dir).parent / "current.npz"
learning_rate = env["learning"]["learning_rate"]
trace_delay = env["learning"]["trace_delay"]

if WEIGHTS_PATH.exists():
    try:
        saved = np.load(WEIGHTS_PATH)
        w, b = saved["w"], saved["b"]
        if w.shape != (n_actions, N_FEATURES) or b.shape != (n_actions,):
            print("Saved policy doesn't match current inputs/neurons — starting fresh")
            w = np.zeros((n_actions, N_FEATURES), dtype=np.float64)
            b = np.zeros(n_actions, dtype=np.float64)
        else:
            print("Loaded trained policy")
    except ValueError:
        print("Invalid data in policy")
        w = np.zeros((n_actions, N_FEATURES), dtype=np.float64)
        b = np.zeros(n_actions, dtype=np.float64)
    except EOFError:
        print("No data in policy")
        w = np.zeros((n_actions, N_FEATURES), dtype=np.float64)
        b = np.zeros(n_actions, dtype=np.float64)
    except BadZipfile:
        print("Invalid data in policy (Not a zip)")
        w = np.zeros((n_actions, N_FEATURES), dtype=np.float64)
        b = np.zeros(n_actions, dtype=np.float64)
else:
    w = np.zeros((n_actions, N_FEATURES), dtype=np.float64)
    b = np.zeros(n_actions, dtype=np.float64)

e_w = np.zeros_like(w)
e_b = np.zeros_like(b)
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
pressed_state = [False] * n_actions
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
            should_press = rng.random(n_actions) < p

            e_w = trace_delay * e_w + np.outer(should_press.astype(np.float64) - p, x)
            e_b = trace_delay * e_b + (should_press.astype(np.float64) - p)

            for i, action in enumerate(actions):
                if should_press[i] and not pressed_state[i]:
                    tx_bytes[i] = action["pressed"]
                    pressed_state[i] = True
                elif not should_press[i] and pressed_state[i]:
                    tx_bytes[i] = action["released"]
                    pressed_state[i] = False

            sync_markers[1] = last_seen_frame

        trainer_seq = int(trainer_markers[0])
        if trainer_seq != last_trainer_seq:
            last_trainer_seq = trainer_seq
            reward = float(trainer_markers[1])

            w += learning_rate * reward * e_w
            b += learning_rate * reward * e_b
            e_w[:] = 0.0
            e_b[:] = 0.0

            np.savez(WEIGHTS_PATH, w=w, b=b)
            print(f"{'Rewarded' if reward > 0 else 'Penalized'} — policy saved")

        time.sleep(0.001)
except KeyboardInterrupt:
    pass
finally:
    np.savez(WEIGHTS_PATH, w=w, b=b)
    del rx_frame, tx_bytes, sync_markers, trainer_markers
    del py_to_rust_buf, lane_2_buf, control_flags, trainer_flags
    shm.close()
    shm.unlink()
    print("\nShared memory unlinked cleanly.")