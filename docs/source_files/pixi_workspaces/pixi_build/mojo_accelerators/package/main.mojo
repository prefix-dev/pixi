from std.algorithm import vectorize
from std.collections import List
from std.math import ceildiv, iota
from std.memory import Pointer
from std.sys import (
    argv,
    default_accelerator,
    get_defined_string,
    has_accelerator,
    simd_width_of,
)
from std.testing import assert_equal
from std.time import perf_counter_ns

from max.algorithm import parallelize
from max.gpu import global_idx
from max.gpu.host import DeviceAttribute, DeviceContext

comptime ACCELERATOR = get_defined_string["mojo_accelerator"]()
comptime CPU_WORKERS = 8
comptime CPU_LANES = simd_width_of[DType.float32]()
comptime BLOCK_SIZE = 256
comptime WARMUPS = 2


@fieldwise_init
struct Config(ImplicitlyCopyable):
    var width: Int
    var height: Int
    var max_iterations: Int
    var repetitions: Int
    var scale_x: Float32
    var scale_y: Float32


@inline(.always)
def escape_counts[lanes: Int](
    cx: SIMD[DType.float32, lanes],
    cy: SIMD[DType.float32, lanes],
    max_iterations: Int,
) -> SIMD[DType.int32, lanes]:
    var real = SIMD[DType.float32, lanes](0)
    var imag = SIMD[DType.float32, lanes](0)
    var counts = SIMD[DType.int32, lanes](0)
    var active = SIMD[DType.bool, lanes](fill=True)
    for _ in range(max_iterations):
        var real_squared = real * real
        var imag_squared = imag * imag
        # Escaped lanes stay inactive, even after their state is frozen.
        active = active & (real_squared + imag_squared).le(4)
        if not active.reduce_or():
            break
        counts = active.select(counts + 1, counts)
        var next_real = (real_squared - imag_squared) + cx
        var next_imag = ((real + real) * imag) + cy
        real = active.select(next_real, real)
        imag = active.select(next_imag, imag)
    return counts


def scalar_reference(cx: Float32, cy: Float32, limit: Int) -> Int32:
    # Deliberately independent control flow, not escape_counts[1].
    var real = Float32(0)
    var imag = Float32(0)
    var count = 0
    while count < limit:
        var real_squared = real * real
        var imag_squared = imag * imag
        if real_squared + imag_squared > Float32(4):
            break
        var next_real = (real_squared - imag_squared) + cx
        imag = ((real + real) * imag) + cy
        real = next_real
        count += 1
    return Int32(count)


def reference_image(config: Config) raises -> List[Int32]:
    assert_equal(scalar_reference(0, 0, 8), Int32(8))
    assert_equal(scalar_reference(-1, 0, 8), Int32(8))
    assert_equal(scalar_reference(-2, 0, 8), Int32(8))
    assert_equal(scalar_reference(1, 0, 8), Int32(3))
    assert_equal(scalar_reference(2, 0, 8), Int32(2))
    assert_equal(scalar_reference(0, 2, 8), Int32(2))
    assert_equal(scalar_reference(3, 0, 8), Int32(1))
    var result = List[Int32](length=config.width * config.height, fill=0)
    for row in range(config.height):
        var cy = Float32(-1) + Float32(row) * config.scale_y
        for col in range(config.width):
            var cx = Float32(-2) + Float32(col) * config.scale_x
            result[row * config.width + col] = scalar_reference(
                cx, cy, config.max_iterations
            )
    return result^


def validate(values: List[Int32], expected: List[Int32]) raises:
    assert_equal(len(values), len(expected))
    for index in range(len(expected)):
        if values[index] != expected[index]:
            print(
                "mismatch index=", index,
                " expected=", expected[index], " actual=", values[index],
                sep="",
            )
            raise Error("Image differs from the independent scalar reference")


def render_cpu[origin: MutOrigin](
    output: Pointer[Int32, origin],
    config: Config,
    ctx: DeviceContext.CPU,
):
    def worker(worker_index: Int) {imm}:
        # Strided rows balance the expensive center across all eight workers.
        for row in range(worker_index, config.height, CPU_WORKERS):
            var cy = Float32(-1) + Float32(row) * config.scale_y

            def columns[lanes: Int](col: Int) {imm}:
                var cx = Float32(-2) + (
                    Float32(col) + iota[DType.float32, lanes]()
                ) * config.scale_x
                var counts = escape_counts[lanes](
                    cx, SIMD[DType.float32, lanes](cy), config.max_iterations
                )
                output.unsafe_store[width=lanes](
                    row * config.width + col, counts
                )

            # The scalar-tail overload also covers widths smaller than CPU_LANES.
            vectorize[CPU_LANES](config.width, columns)

    parallelize(worker, CPU_WORKERS, CPU_WORKERS, ctx)


def render_gpu(
    output: Pointer[Int32, MutAnyOrigin],
    width_arg: Int64,
    height_arg: Int64,
    max_iterations: Int64,
    scale_x: Float32,
    scale_y: Float32,
):
    var width = Int(width_arg)
    var height = Int(height_arg)
    var index = global_idx.x
    if index >= width * height:
        return
    var row = index // width
    var col = index % width
    var cx = Float32(-2) + Float32(col) * scale_x
    var cy = Float32(-1) + Float32(row) * scale_y
    output[unsafe_offset=index] = escape_counts[1](cx, cy, Int(max_iterations))


def cpu_image(config: Config) -> List[Int32]:
    var ctx = DeviceContext.cpu()
    var result = List[Int32](length=config.width * config.height, fill=0)
    render_cpu(result.unsafe_ptr(), config, ctx)
    return result^


def gpu_image(config: Config) raises -> List[Int32]:
    var ctx = DeviceContext()
    var device = ctx.enqueue_create_buffer[DType.int32](
        config.width * config.height
    )
    var result = List[Int32](length=config.width * config.height, fill=0)
    ctx.enqueue_function[render_gpu](
        device, Int64(config.width), Int64(config.height), Int64(config.max_iterations),
        config.scale_x, config.scale_y,
        grid_dim=ceildiv(config.width * config.height, BLOCK_SIZE),
        block_dim=BLOCK_SIZE,
    )
    ctx.enqueue_copy(result.unsafe_ptr(), device)
    ctx.synchronize()
    return result^


def run_cpu(config: Config, expected: List[Int32]) raises:
    var ctx = DeviceContext.cpu()
    var pool_threads = ctx.get_attribute(DeviceAttribute.PARALLELISM_LEVEL)
    print(
        "execution scheduled_workers=", CPU_WORKERS,
        " pool_threads=", pool_threads, " simd_lanes=", CPU_LANES,
        " block_size=0", sep="",
    )
    var output = List[Int32](length=config.width * config.height, fill=0)
    for _ in range(WARMUPS):
        render_cpu(output.unsafe_ptr(), config, ctx)
        validate(output, expected)
        var complete = cpu_image(config)
        validate(complete, expected)
    for repetition in range(config.repetitions):
        var start = perf_counter_ns()
        render_cpu(output.unsafe_ptr(), config, ctx)
        var kernel_ns = perf_counter_ns() - start
        validate(output, expected)
        start = perf_counter_ns()
        var complete = cpu_image(config)
        var e2e_ns = perf_counter_ns() - start
        validate(complete, expected)
        print(
            "sample repetition=", repetition + 1,
            " kernel_ns=", kernel_ns, " e2e_ns=", e2e_ns, sep="",
        )


def run_gpu(config: Config, expected: List[Int32]) raises:
    var ctx = DeviceContext()
    print("device=", ctx.name(), sep="")
    print(
        "execution scheduled_workers=0 pool_threads=0 simd_lanes=1 block_size=",
        BLOCK_SIZE, sep="",
    )
    var device = ctx.enqueue_create_buffer[DType.int32](
        config.width * config.height
    )
    var host = List[Int32](length=config.width * config.height, fill=0)
    for sample in range(WARMUPS + config.repetitions):
        ctx.synchronize()
        var start = perf_counter_ns()
        ctx.enqueue_function[render_gpu](
            device, Int64(config.width), Int64(config.height), Int64(config.max_iterations),
            config.scale_x, config.scale_y,
            grid_dim=ceildiv(config.width * config.height, BLOCK_SIZE),
            block_dim=BLOCK_SIZE,
        )
        ctx.synchronize()
        var kernel_ns = perf_counter_ns() - start
        # Copy and validate every preallocated-buffer result outside kernel time.
        ctx.enqueue_copy(host.unsafe_ptr(), device)
        ctx.synchronize()
        validate(host, expected)
        start = perf_counter_ns()
        var complete = gpu_image(config)
        var e2e_ns = perf_counter_ns() - start
        validate(complete, expected)
        if sample >= WARMUPS:
            print(
                "sample repetition=", sample - WARMUPS + 1,
                " kernel_ns=", kernel_ns, " e2e_ns=", e2e_ns, sep="",
            )


def main() raises:
    var args = argv()
    if len(args) != 1 and len(args) != 5 and len(args) != 6:
        raise Error(
            "Usage: mojo-fractal [width height max_iterations repetitions [cpu|gfx1103]]"
        )
    var width = 1024
    var height = 1024
    var iterations = 512
    var repetitions = 5
    var expected_path = String("any")
    if len(args) >= 5:
        width = Int(String(args[1]))
        height = Int(String(args[2]))
        iterations = Int(String(args[3]))
        repetitions = Int(String(args[4]))
    if width <= 0 or height <= 0 or iterations <= 0 or repetitions <= 0:
        raise Error("Dimensions, iterations and repetitions must be positive")
    if width > 9223372036854775807 // height // 4 or iterations > 2147483647:
        raise Error("Image allocation or iteration count exceeds integer limits")
    if len(args) == 6:
        expected_path = String(args[5])
        if expected_path != ACCELERATOR:
            raise Error("Installed package does not match the expected path")
    var config = Config(
        width, height, iterations, repetitions,
        Float32(3) / Float32(width), Float32(2) / Float32(height),
    )
    print(
        "config path=", ACCELERATOR, " expect_path=", expected_path,
        " width=", width, " height=", height,
        " max_iterations=", iterations, " repetitions=", repetitions,
        " warmups=", WARMUPS,
        " dtype=float32 output_dtype=int32 layout=row_major",
        " min_x=-2 max_x=1 min_y=-1 max_y=1 pixel_sampling=lower_edge",
        " scale_x=", config.scale_x, " scale_y=", config.scale_y,
        " fp_mode=contract_off target_cpu=x86-64-v3 target_features=baseline",
        sep="",
    )
    print(
        "timing clock=perf_counter_ns kernel=dispatch_through_completion",
        " e2e=context_allocation_render_copy_host_owned",
        " startup=excluded warmup_compilation=excluded validation=excluded"
    )
    var expected = reference_image(config)
    comptime if ACCELERATOR == "cpu":
        comptime assert not has_accelerator(), "CPU build detected an accelerator"
        run_cpu(config, expected)
    elif ACCELERATOR == "gfx1103":
        comptime assert default_accelerator().is_amd_gpu["gfx1103"](), "Wrong AMD target"
        run_gpu(config, expected)
    else:
        comptime assert False, "Unsupported accelerator"
    var checksum = Int64(0)
    for value in expected:
        checksum += Int64(value)
    print(
        "validation status=ok pixels=", len(expected),
        " images=", 2 * (WARMUPS + repetitions),
        " reference=independent_scalar exact=true checksum=", checksum,
        sep="",
    )
