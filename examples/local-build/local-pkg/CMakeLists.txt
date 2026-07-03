cmake_minimum_required(VERSION 3.22)

project(local_build_local_pkg C)

add_executable(hello-pkg hello.c)
install(TARGETS hello-pkg DESTINATION bin)
