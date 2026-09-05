# -*- cmake -*-

if (BUILD_VIEWER)
  add_executable(llui_libtest
    llui_libtest.cpp
    llui_libtest.h
    llwidgetreg.cpp
    llwidgetreg.h
  )

  target_compile_definitions(llui_libtest
    PRIVATE
    LLUI_LIBTEST_NEWVIEW_PATH="${INDRA_SOURCE_DIR}/newview"
  )

  target_link_libraries(llui_libtest
    PRIVATE
    llui
    llinventory
    llmessage
    llrender
    llimage
    llimagej2coj
  )

  if (USE_KDU)
    target_link_libraries(llui_libtest PRIVATE llkdu)
  endif ()

  set_target_properties(llui_libtest
    PROPERTIES
    FOLDER "Tests/Integration"
  )

  add_dependencies(${VIEWER_BINARY_NAME} llui_libtest)
  if (TARGET BUILD_TESTS)
    add_dependencies(BUILD_TESTS llui_libtest)
  endif ()

  LL_TEST_LIBRARY_PATH(llui_libtest_library_path)
  LL_TEST_COMMAND(llui_libtest_command
    "${llui_libtest_library_path}"
    "$<TARGET_FILE:llui_libtest>"
  )
  add_test(NAME llui_libtest COMMAND ${llui_libtest_command})
  set_tests_properties(llui_libtest
    PROPERTIES
    LABELS integration
    WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
  )
endif ()

add_executable(llimage_libtest
  llimage_libtest.cpp
  llimage_libtest.h
)

target_link_libraries(llimage_libtest
  PRIVATE
  llcommon
  llfilesystem
  llmath
  llimage
)

set_target_properties(llimage_libtest
  PROPERTIES
  FOLDER "Tests/Integration"
)

if (BUILD_VIEWER)
  add_dependencies(${VIEWER_BINARY_NAME} llimage_libtest)
endif ()
if (TARGET BUILD_TESTS)
  add_dependencies(BUILD_TESTS llimage_libtest)
endif ()

LL_TEST_LIBRARY_PATH(llimage_libtest_library_path)
LL_TEST_COMMAND(llimage_libtest_command
  "${llimage_libtest_library_path}"
  "$<TARGET_FILE:llimage_libtest>"
  "--help"
)
add_test(NAME llimage_libtest COMMAND ${llimage_libtest_command})
set_tests_properties(llimage_libtest
  PROPERTIES
  LABELS integration
  WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
)
