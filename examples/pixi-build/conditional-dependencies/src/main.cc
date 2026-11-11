#include <cstdio>

#if defined(__clang__)
#  define COMPILER "Clang " __clang_version__
#elif defined(__GNUC__)
#  define COMPILER "GCC " __VERSION__
#elif defined(_MSC_VER)
#  define COMPILER "MSVC"
#else
#  define COMPILER "an unknown compiler"
#endif

#ifdef HAS_CUDA
#  include <cuda_runtime.h>
#endif

int main() {
    std::printf("cuda_probe built with %s\n", COMPILER);

#ifdef HAS_CUDA
    int devices = 0;
    cudaError_t err = cudaGetDeviceCount(&devices);
    if (err == cudaSuccess) {
        std::printf("CUDA %s runtime linked, %d device(s) visible\n",
                    CUDA_VERSION_STRING, devices);
    } else {
        std::printf("CUDA %s runtime linked, but no devices (%s)\n",
                    CUDA_VERSION_STRING, cudaGetErrorString(err));
    }
#else
    std::printf("Built without CUDA support (CPU-only)\n");
#endif

    return 0;
}
