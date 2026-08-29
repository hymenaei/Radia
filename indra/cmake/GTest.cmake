# -*- cmake -*-

include_guard()

if(NOT BUILD_TESTING)
  return()
endif()

find_package(GTest CONFIG REQUIRED)

include(GoogleTest)
include(LLTestCommand)

function(_CONFIGURE_GTEST_TARGET target label)
  if(NOT TARGET "${target}")
    message(FATAL_ERROR
      "_CONFIGURE_GTEST_TARGET requires an existing target: ${target}")
  endif()

  if(NOT label)
    message(FATAL_ERROR
      "_CONFIGURE_GTEST_TARGET requires a CTest label")
  endif()

  set(solution_folder "${label}")
  string(SUBSTRING "${solution_folder}" 0 1 first_character)
  string(TOUPPER "${first_character}" first_character)
  string(SUBSTRING "${solution_folder}" 1 -1 remaining_characters)
  set(solution_folder "${first_character}${remaining_characters}")

  target_include_directories(${target}
    PRIVATE
    ${INDRA_SOURCE_DIR}/test
  )

  set_target_properties(${target}
    PROPERTIES
    FOLDER "Tests/${solution_folder}"
    RUNTIME_OUTPUT_DIRECTORY "${EXE_STAGING_DIR}"
  )

  if(WINDOWS)
    target_link_options(${target}
      PRIVATE
      $<$<CONFIG:Release>:/DEBUG:NONE>
    )
  elseif(DARWIN)
    set_target_properties(${target}
      PROPERTIES
      BUILD_WITH_INSTALL_RPATH 1
      INSTALL_RPATH "@executable_path/Frameworks"
      XCODE_ATTRIBUTE_CODE_SIGN_IDENTITY "-"
    )
  elseif(LINUX)
    set_property(TARGET ${target} APPEND PROPERTY
      BUILD_RPATH "${SHARED_LIB_STAGING_DIR}")
  endif()

  if(TARGET stage_third_party_libs)
    add_dependencies(${target} stage_third_party_libs)
  endif()

  if(TARGET BUILD_TESTS)
    add_dependencies(BUILD_TESTS ${target})
  endif()

  LL_TEST_LIBRARY_PATH(test_library_path)
  LL_TEST_LAUNCHER(test_launcher "${test_library_path}")

  if(CMAKE_VERSION VERSION_GREATER_EQUAL 3.29)
    set_property(TARGET ${target} PROPERTY
      TEST_LAUNCHER "${test_launcher}")
  elseif(NOT CMAKE_CROSSCOMPILING)
    # CMake 3.27-3.28 do not support TEST_LAUNCHER. The older
    # CROSSCOMPILING_EMULATOR property is still consumed by the GoogleTest
    # module on the native builds supported by this project.
    set_property(TARGET ${target} PROPERTY
      CROSSCOMPILING_EMULATOR "${test_launcher}")
  endif()

  set(runtime_environment_variable PATH)
  if(DARWIN OR LINUX)
    set(runtime_environment_variable LD_LIBRARY_PATH)
  endif()

  set(runtime_environment_modification
    "${runtime_environment_variable}=path_list_prepend:$<TARGET_FILE_DIR:${target}>")
  foreach(path IN LISTS test_library_path)
    string(APPEND runtime_environment_modification
      ";${runtime_environment_variable}=path_list_prepend:${path}")
  endforeach()

  gtest_discover_tests(${target}
    DISCOVERY_MODE PRE_TEST
    DISCOVERY_TIMEOUT 30
    WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
    PROPERTIES
    LABELS "${label}"
    ENVIRONMENT_MODIFICATION "${runtime_environment_modification}"
    ENVIRONMENT "GTEST_BRIEF=1"
  )
endfunction()
