cmake_minimum_required(VERSION 3.22)
project(slice_fixtures LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_BUILD_TYPE RelWithDebInfo CACHE STRING "")

function(add_slice_fixture target)
  add_executable(${target} ${ARGN})
  target_compile_options(${target} PRIVATE -g -O1 -fno-omit-frame-pointer -fno-optimize-sibling-calls -Wall -Wextra -Wpedantic)
  target_link_options(${target} PRIVATE -rdynamic)
endfunction()

add_slice_fixture(tail_divergence tail_divergence.cpp)
add_slice_fixture(bimodal_service bimodal_service.cpp)
add_slice_fixture(off_cpu_wait off_cpu_wait.cpp)
add_slice_fixture(nested_population nested_population.cpp)

enable_testing()
add_test(NAME tail_divergence_runs COMMAND tail_divergence 100 1)
add_test(NAME bimodal_service_runs COMMAND bimodal_service --iterations 10 --workers 2)
add_test(NAME off_cpu_wait_runs COMMAND off_cpu_wait 8)
add_test(NAME nested_population_runs COMMAND nested_population 4)
