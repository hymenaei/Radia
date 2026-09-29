include(GoogleTest)
include(LLTestCommand)
include(RadiaMacros)

function(_RADIA_REGISTER_GTEST_TARGET target)
    if(NOT TARGET "${target}")
        message(FATAL_ERROR "_RADIA_REGISTER_GTEST_TARGET requires a declared target: ${target}")
    endif()

    set(test_properties)

    if(ARGC GREATER 1 AND ARGV1)
        set(test_properties PROPERTIES LABELS "${ARGV1}")
    endif()

    target_include_directories(${target} PRIVATE ${INDRA_SOURCE_DIR}/test)

    LL_TEST_LIBRARY_PATH(test_library_path)
    LL_TEST_LAUNCHER(test_launcher "${test_library_path}")

    if(CMAKE_VERSION VERSION_GREATER_EQUAL 3.29)
        set_property(TARGET ${target} PROPERTY TEST_LAUNCHER "${test_launcher}")
    elseif(NOT CMAKE_CROSSCOMPILING)
        set_property(TARGET ${target} PROPERTY CROSSCOMPILING_EMULATOR "${test_launcher}")
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
        DISCOVERY_MODE POST_BUILD
        DISCOVERY_TIMEOUT 30
        WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
        ${test_properties}
        ENVIRONMENT_MODIFICATION "${runtime_environment_modification}"
        ENVIRONMENT "GTEST_BRIEF=1"
    )
endfunction()

function(_CONFIGURE_GTEST_TARGET target label)
    if(NOT TARGET "${target}")
        message(FATAL_ERROR "_CONFIGURE_GTEST_TARGET requires an existing target: ${target}")
    endif()

    if(NOT label)
        message(FATAL_ERROR "_CONFIGURE_GTEST_TARGET requires a CTest label")
    endif()

    _RADIA_CONFIGURE_EXECUTABLE(${target})

    if(TARGET BUILD_TESTS)
        add_dependencies(BUILD_TESTS ${target})
    endif()

    string(SUBSTRING "${label}" 0 1 first_character)
    string(TOUPPER "${first_character}" first_character)
    string(SUBSTRING "${label}" 1 -1 remaining_characters)
    set_target_properties(${target} PROPERTIES FOLDER "Tests/${first_character}${remaining_characters}")

    _RADIA_REGISTER_GTEST_TARGET(${target} "${label}")
endfunction()

function(RADIA_TEST target)
    if(NOT BUILD_TESTING)
        return()
    endif()

    RADIA_EXECUTABLE_DECLARE(${target})
    _RADIA_CONFIGURE_EXECUTABLE(${target})

    if(TARGET BUILD_TESTS)
        add_dependencies(BUILD_TESTS ${target})
    endif()

    _RADIA_REGISTER_GTEST_TARGET(${target})
endfunction()
