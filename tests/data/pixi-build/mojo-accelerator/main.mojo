from std.sys import get_defined_string, has_accelerator, default_accelerator
from std.testing import assert_equal
from max.gpu.host import DeviceContext
from std.memory import Pointer

comptime accelerator = get_defined_string["mojo_accelerator"]()

def kernel(output: Pointer[Int32, MutAnyOrigin]):
    output[unsafe_offset=0] = 332833500

def main() raises:
    comptime if accelerator == "cpu":
        comptime assert not has_accelerator(), "CPU build must not detect an accelerator"
        var result = 0
        for i in range(1000):
            result += i * i
        assert_equal(result, 332833500)
        print("path=cpu result=", result, sep="")
    else:
        comptime assert default_accelerator().is_amd_gpu[accelerator](), "wrong AMD target"
        var ctx = DeviceContext()
        var output = ctx.enqueue_create_buffer[DType.int32](1)
        ctx.enqueue_function[kernel](output, grid_dim=1, block_dim=1)
        ctx.synchronize()
        with output.map_to_host() as values:
            assert_equal(values[0], 332833500)
            print("path=", accelerator, " device=", ctx.name(), " result=", values[0], sep="")
