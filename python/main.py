import sys
import time
from multiprocessing import shared_memory
import numpy as np

if len(sys.argv) < 3:
    sys.exit(1)

lane_1 = int(sys.argv[1])
lane_2 = 1
control_size = 64  
total = lane_1 + lane_2 + control_size

SHM_NAME = "flybrain_ipc_buffer"

print(f"Initializing buffer layer of {total} bytes...")

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
control_flags  = shm.buf[control_block_offset + 8 : control_block_offset + 24]

rx_frame = np.ndarray((lane_1,), dtype=np.uint8, buffer=py_to_rust_buf)
tx_byte = np.ndarray((1,), dtype=np.uint8, buffer=rust_to_py_buf)
sync_markers = np.ndarray((2,), dtype=np.int64, buffer=control_flags)

try:
    print("Pipeline active. Awaiting compositor frames...")
    last_seen_frame = 0
    
    while True:
        current_rust_frame = sync_markers[0]
        
        if current_rust_frame > last_seen_frame:
            last_seen_frame = current_rust_frame

            tx_byte[0] = 0x20
            
            sync_markers[1] = last_seen_frame
            
            tx_byte[0] = 0x21

        time.sleep(0.001)
except KeyboardInterrupt:
    pass
finally:
    del rx_frame
    del tx_byte
    del sync_markers
    del py_to_rust_buf
    del rust_to_py_buf
    del control_flags

    shm.close()
    shm.unlink()
    print("\nShared memory unlinked cleanly.")
