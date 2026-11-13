# TwPotato input method for IBus: ibus-engine-twpotato (the C shell + libtwime_shell.a) and its component file.
#
#   cmake -S ime/linux/ibus -B BUILD -DCMAKE_BUILD_TYPE=Release -DTWIME_SHELL_LIB=<target>/release/libtwime_shell.a
#   cmake --build BUILD
# System install (packages): configure with -DCMAKE_INSTALL_PREFIX=/usr, then cmake --install BUILD: the executable goes
# to <prefix>/lib/twpotato and the component file, which names it by that path, to <prefix>/share/ibus/component.
# Development and tests: BUILD/twpotato.xml names the executable in the build directory; put it into a directory of
# its own and start ibus-daemon with IBUS_COMPONENT_PATH pointing there (ibus/test/run.sh does that).
cmake_minimum_required(VERSION 3.16)
project(ibus-twpotato VERSION 0.1.9 LANGUAGES C)

find_package(PkgConfig REQUIRED)
pkg_check_modules(IBUS REQUIRED IMPORTED_TARGET ibus-1.0)

set(TWIME_SHELL_LIB "" CACHE FILEPATH "libtwime_shell.a (cargo build --release -p twime-shell)")
set(TWIME_SHELL_INCLUDE "${CMAKE_CURRENT_SOURCE_DIR}/../../crates/twime-shell/include" CACHE PATH "directory of twime_shell.h")
if(NOT EXISTS "${TWIME_SHELL_LIB}")
    message(FATAL_ERROR "TWIME_SHELL_LIB is not a file: '${TWIME_SHELL_LIB}'")
endif()

set(CMAKE_C_STANDARD 11)
set(CMAKE_C_STANDARD_REQUIRED ON)

add_executable(ibus-engine-twpotato src/main.c)
target_compile_options(ibus-engine-twpotato PRIVATE -Wall -Wextra)
target_include_directories(ibus-engine-twpotato PRIVATE "${TWIME_SHELL_INCLUDE}")
# libibus and GLib are linked dynamically (the system's). The other system libraries are what rustc reports as
# native-static-libs of twime-shell (twime_shell.h).
target_link_libraries(ibus-engine-twpotato PRIVATE PkgConfig::IBUS "${TWIME_SHELL_LIB}" gcc_s util rt pthread m dl c)

# The component file of the build directory, and the one that is installed. <exec> is a command line (ibus-daemon
# splits it by the shell's quoting rules) inside XML: the path becomes one single-quoted argument first, then the
# markup characters of the whole are escaped.
function(twpotato_component path output)
    string(REPLACE "'" "'\\''" quoted "${path}")  # ' -> '\'' : end the quoting, an escaped quote, start again
    set(TWPOTATO_EXEC "'${quoted}' --ibus")
    string(REPLACE "&" "&amp;" TWPOTATO_EXEC "${TWPOTATO_EXEC}")
    string(REPLACE "<" "&lt;" TWPOTATO_EXEC "${TWPOTATO_EXEC}")
    string(REPLACE ">" "&gt;" TWPOTATO_EXEC "${TWPOTATO_EXEC}")
    string(REPLACE "\"" "&quot;" TWPOTATO_EXEC "${TWPOTATO_EXEC}")
    string(REPLACE "'" "&apos;" TWPOTATO_EXEC "${TWPOTATO_EXEC}")
    configure_file(twpotato.xml.in "${output}" @ONLY)
endfunction()
twpotato_component("${CMAKE_CURRENT_BINARY_DIR}/ibus-engine-twpotato" twpotato.xml)
twpotato_component("${CMAKE_INSTALL_PREFIX}/lib/twpotato/ibus-engine-twpotato" install/twpotato.xml)

install(TARGETS ibus-engine-twpotato DESTINATION lib/twpotato)
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/install/twpotato.xml" DESTINATION share/ibus/component)
