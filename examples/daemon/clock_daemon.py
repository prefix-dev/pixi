import time
import argparse
import sys
import logging

logging.basicConfig(level=logging.INFO)

parser = argparse.ArgumentParser()
parser.add_argument("--name", type=str, required=True)
parser.add_argument("-n", type=int, required=True)
parser.add_argument("--sleep", type=int, required=True)
args = parser.parse_args()

print(f"Python Interpreter: {sys.executable}")
print(f"Name: {args.name} - N: {args.n} - Sleep: {args.sleep}")
n = args.n
sleep_seconds = args.sleep

for i in range(n):
    logging.info(f"{i} - {time.ctime()}")
    time.sleep(sleep_seconds)

print("Done")
