cmake_minimum_required(VERSION 3.18)
project(cuda_probe LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_RUNTIME_OUTPUT_DIRECTORY "${CMAKE_BINARY_DIR}/bin")

add_executable(cuda_probe src/main.cc)

# The CUDA runtime is only pulled in as a host-dependency on platforms where
# conda-forge ships it (see the `if(...)` block in pixi.toml). When it is
# present we light up the CUDA code path; otherwise we build a CPU-only binary
# from the exact same sources.
find_package(CUDAToolkit QUIET)
if(CUDAToolkit_FOUND)
    message(STATUS "CUDA toolkit ${CUDAToolkit_VERSION} found, enabling CUDA support")
    target_compile_definitions(cuda_probe PRIVATE
        HAS_CUDA
        CUDA_VERSION_STRING="${CUDAToolkit_VERSION}")
    target_link_libraries(cuda_probe PRIVATE CUDA::cudart)
else()
    message(STATUS "No CUDA toolkit found, building CPU-only")
endif()

include(GNUInstallDirs)
install(TARGETS cuda_probe RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
