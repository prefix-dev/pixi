"""Wait until the simulator accepts connections."""

import os
import socket
import sys
import time

PORT = int(os.environ.get("SIMULATOR_PORT", "8765"))
deadline = time.monotonic() + 30

while time.monotonic() < deadline:
    try:
        socket.create_connection(("127.0.0.1", PORT), timeout=1).close()
    except OSError:
        time.sleep(0.2)
    else:
        print("wait: the simulator is up")
        sys.exit(0)

print("wait: the simulator did not start in time", file=sys.stderr)
sys.exit(1)
