cmake_minimum_required(VERSION 3.20)
project(artifact_key_repro LANGUAGES NONE)
file(WRITE "${CMAKE_CURRENT_BINARY_DIR}/repro-value.txt" "$ENV{REPRO_VALUE}\n")
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/repro-value.txt" DESTINATION share/artifact-key-repro)
