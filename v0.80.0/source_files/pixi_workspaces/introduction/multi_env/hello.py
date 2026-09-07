from sys import version_info as vi
from cowpy.cow import Cowacter

python_version = f"Python {vi.major}.{vi.minor}"
message = Cowacter().milk(f"Hello from {python_version}!")
print(message)
