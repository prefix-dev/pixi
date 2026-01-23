cmake_minimum_required(VERSION 3.20)
project(env-target-test-cmake-app)

# Print environment variables during build
message(STATUS "GLOBAL_ENV_VAR=$ENV{GLOBAL_ENV_VAR}")
message(STATUS "UNIX_SPECIFIC_VAR=$ENV{UNIX_SPECIFIC_VAR}")
message(STATUS "WIN_SPECIFIC_VAR=$ENV{WIN_SPECIFIC_VAR}")
message(STATUS "PLATFORM_TYPE=$ENV{PLATFORM_TYPE}")

# Minimal install target
install(CODE "message(\"Install completed\")")