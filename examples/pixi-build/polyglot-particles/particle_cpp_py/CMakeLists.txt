cmake_minimum_required(VERSION 3.20)
project(particle_cpp_py CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Python REQUIRED COMPONENTS Interpreter Development.Module)
find_package(pybind11 CONFIG REQUIRED)
find_package(particle_core REQUIRED)
find_package(particle_cpp REQUIRED)

pybind11_add_module(_native MODULE src/module.cc)

target_link_libraries(_native PRIVATE
    particle_core::particle_core
    particle_cpp::particle_cpp)

# Install the extension and the Python wrapper next to each other in the
# environment's site-packages. Python_SITEARCH is absolute and lives inside
# the conda host prefix, so rattler-build picks the files up with the right
# relative paths.
set(_pkg_dir "${Python_SITEARCH}/particle_cpp_py")
install(TARGETS _native DESTINATION "${_pkg_dir}")
install(FILES src/particle_cpp_py/__init__.py DESTINATION "${_pkg_dir}")
