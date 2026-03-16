cmake_minimum_required(VERSION 3.20)
project(simple_cpp)

add_executable(${PROJECT_NAME} src/main.cc)

install(TARGETS ${PROJECT_NAME} DESTINATION bin)