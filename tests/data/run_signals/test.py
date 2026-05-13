# pyright: basic

import sys
import signal
import time
from pathlib import Path

output_file = Path("output.txt")
if output_file.exists():
    output_file.unlink()

ready_file = Path("ready.txt")
if ready_file.exists():
    ready_file.unlink()


def signal_handler(signum, frame):
    output_file.write_text(f"Signal handler called with signal {signum}\n")
    if signum == signal.SIGINT:
        print("SIGINT received, exiting gracefully...")
        output_file.write_text("SIGINT received, exiting gracefully...\n")
        sys.exit(12)


signal.signal(signal.SIGINT, signal_handler)

# Signal to the test driver that the SIGINT handler is now installed. Without
# this, the driver might deliver SIGINT before this line is reached, in which
# case the default handler kills Python with exit code 130.
ready_file.write_text("ready\n")

while True:
    print("Running...\n")
    time.sleep(1)
