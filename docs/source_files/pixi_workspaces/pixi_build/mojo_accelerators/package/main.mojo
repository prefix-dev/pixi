from std.collections import List
from std.math import ceildiv
from std.memory import Pointer
from std.sys import get_defined_string

from max.gpu import global_idx
from max.gpu.host import DeviceContext

comptime ACCELERATOR = get_defined_string["mojo_accelerator"]()
comptime SIDE = 1024
comptime BLOCK_SIZE = 256


def dot[lhs_origin: MutOrigin, rhs_origin: MutOrigin](
    lhs: Pointer[Float32, lhs_origin],
    rhs: Pointer[Float32, rhs_origin],
    row: Int,
    col: Int,
) -> Float32:
    var value = Float32(0)
    for k in range(SIDE):
        value += lhs[unsafe_offset=row * SIDE + k] * rhs[unsafe_offset=k * SIDE + col]
    return value


def multiply(
    lhs: Pointer[Float32, MutAnyOrigin],
    rhs: Pointer[Float32, MutAnyOrigin],
    output: Pointer[Float32, MutAnyOrigin],
):
    var i = global_idx.x
    if i < SIDE * SIDE:
        output[unsafe_offset=i] = dot(lhs, rhs, i // SIDE, i % SIDE)


def main() raises:
    var lhs = List[Float32](length=SIDE * SIDE, fill=1)
    var rhs = List[Float32](length=SIDE * SIDE, fill=2)
    var output = List[Float32](unsafe_uninit_length=SIDE * SIDE)
    var result = output.unsafe_ptr()
    for i in range(SIDE):
        lhs[i * SIDE + i] += 1
        rhs[i * SIDE + i] += 1

    comptime if ACCELERATOR == "cpu":
        for row in range(SIDE):
            for col in range(SIDE):
                result[unsafe_offset=row * SIDE + col] = dot(
                    lhs.unsafe_ptr(), rhs.unsafe_ptr(), row, col
                )
    elif ACCELERATOR == "gfx1103":
        var ctx = DeviceContext()
        var device_lhs = ctx.enqueue_create_buffer[DType.float32](SIDE * SIDE)
        var device_rhs = ctx.enqueue_create_buffer[DType.float32](SIDE * SIDE)
        var device_output = ctx.enqueue_create_buffer[DType.float32](SIDE * SIDE)
        ctx.enqueue_copy(device_lhs, lhs.unsafe_ptr())
        ctx.enqueue_copy(device_rhs, rhs.unsafe_ptr())
        ctx.enqueue_function[multiply](
            device_lhs, device_rhs, device_output,
            grid_dim=ceildiv(SIDE * SIDE, BLOCK_SIZE), block_dim=BLOCK_SIZE,
        )
        ctx.enqueue_copy(result, device_output)
        ctx.synchronize()

    for i in range(SIDE * SIDE):
        var expected = Float32(2 * SIDE + 3 + Int(i // SIDE == i % SIDE))
        if output[i] != expected:
            raise Error("Matrix multiplication mismatch at index " + String(i))
    print("path=", ACCELERATOR, " side=", SIDE, " validation=ok", sep="")
