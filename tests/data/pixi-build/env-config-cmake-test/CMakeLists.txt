cmake_minimum_required(VERSION 3.20)
project(env-test-cmake-app)

# Print environment variables during build
message(STATUS "CUSTOM_BUILD_VAR=$ENV{CUSTOM_BUILD_VAR}")
message(STATUS "PIXI_TEST_ENV=$ENV{PIXI_TEST_ENV}")  
message(STATUS "BUILD_MESSAGE=$ENV{BUILD_MESSAGE}")

# Minimal install target
install(CODE "message(\"Install completed\")")