# -*- cmake -*-

include_guard()

if(NOT BUILD_BENCHMARKING)
  return()
endif()

find_package(benchmark CONFIG REQUIRED)

function(_CONFIGURE_BENCHMARK_TARGET target)
  if(NOT TARGET "${target}")
    message(FATAL_ERROR
      "_CONFIGURE_BENCHMARK_TARGET requires an existing target: ${target}")
  endif()

  set_target_properties(${target}
    PROPERTIES
    FOLDER "Tests/Benchmark"
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
endfunction()
