"""A dummy simulator that serves the position of a fake robot over HTTP."""

import os
import signal
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

PORT = int(os.environ.get("SIMULATOR_PORT", "8765"))


class Handler(BaseHTTPRequestHandler):
    position = 0.0

    def do_GET(self) -> None:
        Handler.position += 0.5
        body = f"{Handler.position}".encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: Any) -> None:
        pass


def stop(signum: int, frame: Any) -> None:
    print("simulator: stopped", flush=True)
    sys.exit(0)


signal.signal(signal.SIGTERM, stop)
signal.signal(signal.SIGINT, stop)

print("simulator: loading the world", flush=True)
time.sleep(1)
server = ThreadingHTTPServer(("127.0.0.1", PORT), Handler)
print(f"simulator: ready on port {PORT}", flush=True)
server.serve_forever()
