cmake_minimum_required(VERSION 3.20)
project(particle_core C)

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

add_library(particle_core SHARED src/pool.c)

target_include_directories(particle_core
    PUBLIC
        $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
        $<INSTALL_INTERFACE:include>)

target_compile_definitions(particle_core PRIVATE PC_BUILDING)

set_target_properties(particle_core PROPERTIES
    C_VISIBILITY_PRESET hidden
    POSITION_INDEPENDENT_CODE ON)

include(GNUInstallDirs)

install(TARGETS particle_core
    EXPORT particle_coreTargets
    LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})

install(DIRECTORY include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})

install(EXPORT particle_coreTargets
    FILE particle_coreConfig.cmake
    NAMESPACE particle_core::
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/particle_core)
