cmake_minimum_required(VERSION 3.20)
project(env-target-invalid-test-cmake-app)

# Minimal install target
install(CODE "message(\"Install completed\")")