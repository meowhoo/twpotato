# TwPotato input method for Fcitx5: twpotato.so (the C++ shell + libtwime_shell.a) and its two configuration files.
#
#   cmake -S ime/linux/fcitx5 -B BUILD -DCMAKE_BUILD_TYPE=Release -DTWIME_SHELL_LIB=<target>/release/libtwime_shell.a
#   cmake --build BUILD
# System install (packages): configure with -DCMAKE_INSTALL_PREFIX=/usr, then cmake --install BUILD (the distribution's
# Fcitx5 addon directory and share/fcitx5; with another prefix the files go under that prefix).
# User-mode install (development, tests): install-user.sh
cmake_minimum_required(VERSION 3.16)
project(fcitx5-twpotato VERSION 0.1.9 LANGUAGES CXX)

find_package(Fcitx5Core REQUIRED)

set(TWIME_SHELL_LIB "" CACHE FILEPATH "libtwime_shell.a (cargo build --release -p twime-shell)")
set(TWIME_SHELL_INCLUDE "${CMAKE_CURRENT_SOURCE_DIR}/../../crates/twime-shell/include" CACHE PATH "directory of twime_shell.h")
if(NOT EXISTS "${TWIME_SHELL_LIB}")
    message(FATAL_ERROR "TWIME_SHELL_LIB is not a file: '${TWIME_SHELL_LIB}'")
endif()

# C++20: the headers of Fcitx5 5.1.19 (Ubuntu 26.04) need it; 5.1.7 (Ubuntu 24.04) builds with it too.
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_VISIBILITY_PRESET hidden)

add_library(twpotato MODULE src/twpotato.cpp)
set_target_properties(twpotato PROPERTIES PREFIX "")
target_compile_options(twpotato PRIVATE -Wall -Wextra)
target_include_directories(twpotato PRIVATE "${TWIME_SHELL_INCLUDE}")
# The system libraries are what rustc reports as native-static-libs of twime-shell (twime_shell.h). The symbols of the
# static library stay inside the module.
target_link_libraries(twpotato PRIVATE Fcitx5::Core "${TWIME_SHELL_LIB}" gcc_s util rt pthread m dl c)
target_link_options(twpotato PRIVATE -Wl,--exclude-libs,ALL -Wl,--no-undefined)

# Fcitx5 loads <addon directory>/<Library>.so; the user-mode install writes an absolute path here instead.
set(TWPOTATO_ADDON_LIBRARY "twpotato")
configure_file(twpotato-addon.conf.in twpotato-addon.conf @ONLY)

install(TARGETS twpotato DESTINATION "${FCITX_INSTALL_ADDONDIR}")
install(FILES "${CMAKE_CURRENT_BINARY_DIR}/twpotato-addon.conf" RENAME twpotato.conf DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon")
install(FILES twpotato.conf DESTINATION "${FCITX_INSTALL_PKGDATADIR}/inputmethod")
