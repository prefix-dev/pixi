cmake_minimum_required(VERSION 3.20)
project(particle_cpp CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(particle_core REQUIRED)

add_library(particle_cpp SHARED
    src/emitters.cc
    src/modifiers.cc)

target_include_directories(particle_cpp
    PUBLIC
        $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>
        $<INSTALL_INTERFACE:include>)

target_link_libraries(particle_cpp PUBLIC particle_core::particle_core)

target_compile_definitions(particle_cpp PRIVATE PCPP_BUILDING)

set_target_properties(particle_cpp PROPERTIES
    CXX_VISIBILITY_PRESET hidden
    POSITION_INDEPENDENT_CODE ON)

include(GNUInstallDirs)

install(TARGETS particle_cpp
    EXPORT particle_cppTargets
    LIBRARY DESTINATION ${CMAKE_INSTALL_LIBDIR}
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
    RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})

install(DIRECTORY include/ DESTINATION ${CMAKE_INSTALL_INCLUDEDIR})

install(EXPORT particle_cppTargets
    FILE particle_cppConfig.cmake
    NAMESPACE particle_cpp::
    DESTINATION ${CMAKE_INSTALL_LIBDIR}/cmake/particle_cpp)
