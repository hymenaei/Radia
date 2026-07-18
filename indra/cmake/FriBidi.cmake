# -*- cmake -*-

include_guard()

if (NOT TARGET ll::fribidi)
  find_package(PkgConfig REQUIRED)
  pkg_check_modules(FRIBIDI REQUIRED IMPORTED_TARGET GLOBAL fribidi)

  add_library(ll::fribidi INTERFACE IMPORTED)
  target_link_libraries(ll::fribidi INTERFACE PkgConfig::FRIBIDI)
endif()
