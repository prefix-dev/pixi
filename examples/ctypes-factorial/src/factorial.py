import argparse
import ctypes
import sys
from loguru import logger as log

log.remove()
log.add(
    sys.stdout,
    format="{time:YYYY-MM-DD@HH:mm:ss.SSSSSS}|{level}|{name}.{function}:{line}|{message}"
)


def python_factorial(n):
    significand = 1.0
    exponent = 0

    for i in range(2, n + 1):
        significand *= i
        while significand >= 10.0:
            significand /= 10.0
            exponent += 1

    return significand, exponent


def c_factorial(n):
    c_lib = ctypes.CDLL("src/factorial.so")
    c_lib.calculate_factorial_approximation.argtypes = [
        ctypes.c_ulong,
        ctypes.POINTER(ctypes.c_double),
        ctypes.POINTER(ctypes.c_int),
    ]
    c_lib.calculate_factorial_approximation.restype = None

    significand = ctypes.c_double()
    exponent = ctypes.c_int()

    c_lib.calculate_factorial_approximation(n, ctypes.byref(significand), ctypes.byref(exponent))
    return significand.value, exponent.value


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Calculate factorial using Python or C with ctypes."
    )
    parser.add_argument(
        "n",
        type=int,
        nargs='?',
        default=10,
        help="Number for which to calculate the factorial."
    )
    parser.add_argument(
        "-e", "--engine",
        choices=["python", "ctypes"],
        default="ctypes",
        help="Calculation engine (python or ctypes)."
    )
    args = parser.parse_args()

    if not 2 < args.n < 1_000_000_001:
        log.error("no, thank you")
        sys.exit(1)

    if args.engine == "python":
        log.info(f"calculating factorial of {args.n} using pure Python...")
        result = python_factorial(args.n)
    elif args.engine == "ctypes":
        log.info(f"calculating factorial of {args.n} using ctypes...")
        result = c_factorial(args.n)
    else:
        print("Invalid engine choice. Use 'python' or 'ctypes'.")
        result = None

    if result is not None:
        significand, exponent = result
        log.info(f"{args.n}! ≈ {significand:.6f}e{exponent}")