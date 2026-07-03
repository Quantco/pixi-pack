cmake_minimum_required(VERSION 3.22)

project(local_build_main_pkg C)

add_executable(hello-main hello.c)
install(TARGETS hello-main DESTINATION bin)
