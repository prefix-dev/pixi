"""A dummy controller that reads the position of the robot from the simulator."""

import os
import urllib.request

PORT = int(os.environ.get("SIMULATOR_PORT", "8765"))

for _ in range(3):
    with urllib.request.urlopen(f"http://127.0.0.1:{PORT}") as response:
        print(f"controller: robot at x={response.read().decode()}")
