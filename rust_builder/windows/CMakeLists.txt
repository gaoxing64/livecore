# The Flutter tooling requires that developers have a version of Visual Studio
# installed that includes CMake 3.14 or later. You should not increase this
# version, as doing so will cause the plugin to fail to compile for some
# customers of the plugin.
cmake_minimum_required(VERSION 3.14)

# Project-level configuration.
set(PROJECT_NAME "rust_lib_livecore")
project(${PROJECT_NAME} LANGUAGES CXX)

include("../cargokit/cmake/cargokit.cmake")
get_filename_component(LIVECORE_MANIFEST_ROOT "${CMAKE_CURRENT_LIST_DIR}/../.." REALPATH)
get_filename_component(LIVECORE_SOURCE_DIR "${CMAKE_CURRENT_SOURCE_DIR}" REALPATH)
file(RELATIVE_PATH LIVECORE_MANIFEST_DIR "${LIVECORE_SOURCE_DIR}" "${LIVECORE_MANIFEST_ROOT}")
apply_cargokit(${PROJECT_NAME} "${LIVECORE_MANIFEST_DIR}" rust_lib_livecore "")

# List of absolute paths to libraries that should be bundled with the plugin.
# This list could contain prebuilt libraries, or libraries created by an
# external build triggered from this build file.
set(rust_lib_livecore_bundled_libraries
  "${${PROJECT_NAME}_cargokit_lib}"
  PARENT_SCOPE
)
