# pyright: basic

import sys
import signal
import time
from pathlib import Path

output_file = Path("output.txt")
if output_file.exists():
    output_file.unlink()


def signal_handler(signum, frame):
    output_file.write_text(f"Signal handler called with signal {signum}\n")
    if signum == signal.SIGINT:
        print("SIGINT received, exiting gracefully...")
        output_file.write_text("SIGINT received, exiting gracefully...\n")
        sys.exit(12)


signal.signal(signal.SIGINT, signal_handler)

while True:
    print("Running...\n")
    time.sleep(1)
