cmake_minimum_required(VERSION 3.16)
project(package-a VERSION 0.1.0)

# Create an executable from a single source file
add_executable(${PROJECT_NAME} src/main.cpp)

# Set C++ standard
set_target_properties(${PROJECT_NAME} PROPERTIES
    CXX_STANDARD 11
    CXX_STANDARD_REQUIRED ON
)

# Find and link against package_b library
find_package(package-b REQUIRED)
target_link_libraries(${PROJECT_NAME} package-b)

# Install the executable
install(TARGETS ${PROJECT_NAME}
    RUNTIME DESTINATION bin
)
