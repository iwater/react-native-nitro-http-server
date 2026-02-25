cmake_minimum_required(VERSION 3.9.0)
project(RNHttpServer)

set(PACKAGE_NAME RNHttpServer)
set(CMAKE_VERBOSE_MAKEFILE ON)
set(CMAKE_CXX_STANDARD 20)

# Android 16KB page size support (AOSP Android 15+):
# The JNI wrapper .so must be built with 16KB alignment so the app loads on devices
# that use 16KB memory pages. Only arm64-v8a and x86_64 support 16KB; prebuilt
# librn_http_server.so in android/libs/ should also be rebuilt with 16KB alignment.
if(ANDROID_ABI STREQUAL "arm64-v8a" OR ANDROID_ABI STREQUAL "x86_64")
  set(CMAKE_SHARED_LINKER_FLAGS "${CMAKE_SHARED_LINKER_FLAGS} -Wl,-z,max-page-size=16384")
endif()

# Add our custom implementation and JNI adapter
add_library(${PACKAGE_NAME} SHARED
        ../cpp/HybridHttpServer.cpp
        OnLoad.cpp
)

# Include paths for our headers
include_directories(
    ${CMAKE_CURRENT_SOURCE_DIR}/../cpp
    ${CMAKE_CURRENT_SOURCE_DIR}/../nitrogen/generated/android
)

# Include the Nitrogen-generated CMake file
include(${CMAKE_CURRENT_SOURCE_DIR}/../nitrogen/generated/android/RNHttpServer+autolinking.cmake)

# Link the Rust HttpServer library
target_link_directories(${PACKAGE_NAME} PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/libs/${ANDROID_ABI})
target_link_libraries(${PACKAGE_NAME} 
    rn_http_server
    log
)
