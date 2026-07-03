cmake_minimum_required(VERSION 3.22)

project(local_build_local_pkg2 C)

add_executable(hello-pkg2 hello.c)
install(TARGETS hello-pkg2 DESTINATION bin)
