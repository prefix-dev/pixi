cmake_minimum_required(VERSION 3.20...3.27)
project(python_bindings)

# Find Python this should be anything above 3.8
find_package(Python 3.8 COMPONENTS Interpreter Development.Module REQUIRED) # (1)!

# Detect the installed nanobind package and import it into CMake
execute_process(
  COMMAND "${Python_EXECUTABLE}" -m nanobind --cmake_dir
  OUTPUT_STRIP_TRAILING_WHITESPACE OUTPUT_VARIABLE nanobind_ROOT
) # (2)!

# Query the Python for the site-packages directory
execute_process(
    COMMAND ${Python_EXECUTABLE} -c "import sysconfig; print(sysconfig.get_path('purelib'))"
    OUTPUT_VARIABLE PYTHON_SITE_PACKAGES
    OUTPUT_STRIP_TRAILING_WHITESPACE
) # (3)!

# Find the installed nanobind package
find_package(nanobind CONFIG REQUIRED) # (4)!

# Add the extension module
nanobind_add_module(${PROJECT_NAME} src/bindings.cpp) # (5)!

install( # (6)!
    TARGETS ${PROJECT_NAME}
    EXPORT ${PROJECT_NAME}Targets
    LIBRARY DESTINATION ${PYTHON_SITE_PACKAGES}
    ARCHIVE DESTINATION ${CMAKE_INSTALL_LIBDIR}
    RUNTIME DESTINATION ${BINDIR}
)
