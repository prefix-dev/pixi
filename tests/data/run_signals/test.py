# type: ignore
import signal
import time

file = open("./output.txt", "w")


def signal_handler(signum, frame):
    file.write(f"Signal handler called with signal {signum}\n")
    if signum == signal.SIGINT:
        file.write("SIGINT received, exiting gracefully...\n")
        exit(12)
    elif signum == signal.SIGHUP:
        file.write("HUP HUP HUP\n")
        return


signal.signal(signal.SIGINT, signal_handler)
signal.signal(signal.SIGHUP, signal_handler)

while True:
    file.write("Running...\n")
    time.sleep(1)
