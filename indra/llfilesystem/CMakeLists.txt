# -*- cmake -*-

add_library(llfilesystem STATIC)

target_sources(llfilesystem
    PRIVATE
    lldir.cpp
    lldiriterator.cpp
    lllfsthread.cpp
    lldiskcache.cpp
    llfilesystem.cpp
    )

target_sources(llfilesystem
    PUBLIC
    FILE_SET HEADERS
    FILES
    lldir.h
    lldirguard.h
    lldiriterator.h
    lllfsthread.h
    lldiskcache.h
    llfilesystem.h
    )

if (DARWIN)
  target_sources(llfilesystem
      PRIVATE
      lldir_mac.mm
      PUBLIC
      FILE_SET HEADERS
      FILES
      lldir_mac.h
      )
  set_source_files_properties(
    lldir_mac.mm
    PROPERTIES
    SKIP_PRECOMPILE_HEADERS TRUE
  )
endif (DARWIN)

if (LINUX)
  target_sources(llfilesystem
      PRIVATE
      lldir_linux.cpp
      PUBLIC
      FILE_SET HEADERS
      FILES
      lldir_linux.h
      )

  if (INSTALL)
    set_source_files_properties(lldir_linux.cpp
                                PROPERTIES COMPILE_FLAGS
                                "-DAPP_RO_DATA_DIR=\\\"${APP_SHARE_DIR}\\\""
                                )
  endif ()
endif ()

if (WINDOWS)
  target_sources(llfilesystem
      PRIVATE
      lldir_win32.cpp
      PUBLIC
      FILE_SET HEADERS
      FILES
      lldir_win32.h
      )
endif ()

target_link_libraries(llfilesystem
      PUBLIC
      llmath
      llcommon
    )
target_include_directories( llfilesystem  INTERFACE   ${CMAKE_CURRENT_SOURCE_DIR})
target_precompile_headers(llfilesystem REUSE_FROM llprecompiled)

if (BUILD_TESTING)
  add_subdirectory(tests)
endif ()
