# CMake 4 refuses minimum versions below 3.5 unless CMAKE_POLICY_VERSION_MINIMUM is set
cmake_minimum_required(VERSION 3.0)
project(plain_cmake C)

add_executable(plain_cmake main.c)

install(TARGETS plain_cmake DESTINATION lib/${PROJECT_NAME})
