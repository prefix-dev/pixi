import ctypes
import pathlib
import sys

_LIB_NAME = {
    "darwin": "libmathlib.dylib",
    "linux": "libmathlib.so",
    "win32": "mathlib.dll",
}[sys.platform]

_lib = ctypes.CDLL(str(pathlib.Path(__file__).parent / _LIB_NAME))

_lib.add.argtypes = [ctypes.c_int64, ctypes.c_int64]
_lib.add.restype = ctypes.c_int64
add = _lib.add

_lib.fibonacci.argtypes = [ctypes.c_int64]
_lib.fibonacci.restype = ctypes.c_int64
fibonacci = _lib.fibonacci

_lib.gcd.argtypes = [ctypes.c_int64, ctypes.c_int64]
_lib.gcd.restype = ctypes.c_int64
gcd = _lib.gcd


def main():
    print("Zig + Python example")
    print(f"  add(2, 3)        = {add(2, 3)}")
    print(f"  fibonacci(10)    = {fibonacci(10)}")
    print(f"  gcd(12, 8)       = {gcd(12, 8)}")
