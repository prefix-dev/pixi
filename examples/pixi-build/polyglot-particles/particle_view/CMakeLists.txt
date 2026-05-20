cmake_minimum_required(VERSION 3.20)
project(particle_view CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Python REQUIRED COMPONENTS Interpreter Development.Module)
find_package(pybind11 CONFIG REQUIRED)
find_package(particle_core REQUIRED)
find_package(SDL2 REQUIRED)

pybind11_add_module(_native MODULE src/view.cc)

target_link_libraries(_native PRIVATE
    particle_core::particle_core
    SDL2::SDL2)

set(_pkg_dir "${Python_SITEARCH}/particle_view")
install(TARGETS _native DESTINATION "${_pkg_dir}")
install(FILES src/particle_view/__init__.py DESTINATION "${_pkg_dir}")
