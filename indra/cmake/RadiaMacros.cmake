include_guard(GLOBAL)

macro(RADIA_FRAMEWORK_DECLARE target)
    add_library(${target} STATIC)
endmacro()

macro(RADIA_INCLUDE_CONFIG_FILES_IF_EXISTS)
    include("${CMAKE_CURRENT_SOURCE_DIR}/Platform${PORT}.cmake" OPTIONAL)
endmacro()

function(RADIA_COMPUTE_SOURCES target)
    set(codegen_command "${target}_CODEGEN")

    if(COMMAND ${codegen_command})
        cmake_language(CALL "${codegen_command}")
    endif()

    set(sources ${${target}_SOURCES})

    foreach(source_list_file IN LISTS ${target}_SOURCE_LIST_FILES)
        set(source_list_path "${CMAKE_CURRENT_SOURCE_DIR}/${source_list_file}")
        file(STRINGS "${source_list_path}" entries)
        set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${source_list_path}")

        foreach(entry IN LISTS entries)
            string(STRIP "${entry}" entry)

            if(entry STREQUAL "" OR entry MATCHES "^(#|//)")
                continue()
            endif()

            if(IS_ABSOLUTE "${entry}")
                list(APPEND sources "${entry}")
            else()
                list(APPEND sources "${CMAKE_CURRENT_SOURCE_DIR}/${entry}")
            endif()
        endforeach()
    endforeach()

    set(${target}_SOURCES "${sources}" PARENT_SCOPE)
endfunction()

macro(RADIA_EXECUTABLE_DECLARE target)
    add_executable(${target})
endmacro()

function(_RADIA_CONFIGURE_EXECUTABLE target)
    if(NOT TARGET "${target}")
        message(FATAL_ERROR "Cannot configure undeclared executable target: ${target}")
    endif()

    set(sources)

    foreach(source IN LISTS ${target}_SOURCES ${target}_HEADERS ${target}_PRIVATE_HEADERS)
        if(IS_ABSOLUTE "${source}")
            list(APPEND sources "${source}")
        else()
            list(APPEND sources "${CMAKE_CURRENT_SOURCE_DIR}/${source}")
        endif()
    endforeach()

    if(sources)
        target_sources(${target} PRIVATE ${sources})
    endif()

    set(include_directories ${${target}_INCLUDE_DIRECTORIES} ${${target}_PRIVATE_INCLUDE_DIRECTORIES})

    if(include_directories)
        target_include_directories(${target} PRIVATE ${include_directories})
    endif()

    set(libraries ${${target}_LIBRARIES} ${${target}_PRIVATE_LIBRARIES})

    if(libraries)
        target_link_libraries(${target} PRIVATE ${libraries})
    endif()

    set(definitions ${${target}_DEFINITIONS} ${${target}_PRIVATE_DEFINITIONS})

    if(definitions)
        target_compile_definitions(${target} PRIVATE ${definitions})
    endif()

    if(${target}_COMPILE_OPTIONS)
        target_compile_options(${target} PRIVATE ${${target}_COMPILE_OPTIONS})
    endif()

    if(${target}_DEPENDENCIES)
        add_dependencies(${target} ${${target}_DEPENDENCIES})
    endif()

    set_target_properties(${target} PROPERTIES
        FOLDER "Tests"
        RUNTIME_OUTPUT_DIRECTORY "${EXE_STAGING_DIR}"
    )

    if(WINDOWS)
        target_link_options(${target} PRIVATE $<$<CONFIG:Release>:/DEBUG:NONE>)
    elseif(DARWIN)
        set_target_properties(${target} PROPERTIES
            BUILD_WITH_INSTALL_RPATH 1
            INSTALL_RPATH "@executable_path/Frameworks"
            XCODE_ATTRIBUTE_CODE_SIGN_IDENTITY "-"
        )
    elseif(LINUX)
        set_property(TARGET ${target} APPEND PROPERTY BUILD_RPATH "${SHARED_LIB_STAGING_DIR}")
    endif()

    if(TARGET stage_third_party_libs)
        add_dependencies(${target} stage_third_party_libs)
    endif()
endfunction()

function(RADIA_EXECUTABLE target)
    _RADIA_CONFIGURE_EXECUTABLE(${target})
endfunction()

function(RADIA_BENCHMARK target)
    if(NOT BUILD_BENCHMARKING)
        return()
    endif()

    RADIA_EXECUTABLE_DECLARE(${target})
    _RADIA_CONFIGURE_EXECUTABLE(${target})
endfunction()

function(_RADIA_EXPORT_FRAMEWORK_HEADERS target destination_dir)
    file(MAKE_DIRECTORY "${destination_dir}")

    set(header_names)

    foreach(header IN LISTS ARGN)
        get_filename_component(header_name "${header}" NAME)

        if(header_name IN_LIST header_names)
            message(FATAL_ERROR "${target} headers must have unique basenames: ${header_name}")
        endif()

        list(APPEND header_names "${header_name}")

        if(IS_ABSOLUTE "${header}")
            set(header_source "${header}")
        else()
            set(header_source "${CMAKE_CURRENT_SOURCE_DIR}/${header}")
        endif()

        file(RELATIVE_PATH header_include "${destination_dir}" "${header_source}")
        file(TO_CMAKE_PATH "${header_include}" header_include)
        file(GENERATE
            OUTPUT "${destination_dir}/${header_name}"
            CONTENT "#pragma once\n\n#include \"${header_include}\"\n"
        )
    endforeach()
endfunction()

macro(RADIA_FRAMEWORK target)
    set(FRAMEWORK_HEADERS_DIR "${CMAKE_CURRENT_BINARY_DIR}/Headers")
    set(PRIVATE_FRAMEWORK_HEADERS_DIR "${CMAKE_CURRENT_BINARY_DIR}/PrivateHeaders")
    _RADIA_EXPORT_FRAMEWORK_HEADERS(
        "${target}"
        "${FRAMEWORK_HEADERS_DIR}/${target}"
        ${${target}_HEADERS}
    )

    target_sources(${target} PRIVATE
        ${${target}_SOURCES}
        ${${target}_HEADERS}
        ${${target}_PRIVATE_HEADERS}
    )

    set(_radia_framework_include_directories "${FRAMEWORK_HEADERS_DIR}")

    if(${target}_INCLUDE_DIRECTORIES)
        list(APPEND _radia_framework_include_directories ${${target}_INCLUDE_DIRECTORIES})
    endif()

    target_include_directories(${target} PUBLIC ${_radia_framework_include_directories})

    set(_radia_framework_private_include_directories)

    if(${target}_PRIVATE_HEADERS)
        _RADIA_EXPORT_FRAMEWORK_HEADERS(
            "${target}"
            "${PRIVATE_FRAMEWORK_HEADERS_DIR}/${target}"
            ${${target}_PRIVATE_HEADERS}
        )
        list(APPEND _radia_framework_private_include_directories "${PRIVATE_FRAMEWORK_HEADERS_DIR}")
    endif()

    if(${target}_PRIVATE_INCLUDE_DIRECTORIES)
        list(APPEND _radia_framework_private_include_directories ${${target}_PRIVATE_INCLUDE_DIRECTORIES})
    endif()

    if(_radia_framework_private_include_directories)
        target_include_directories(${target} PRIVATE ${_radia_framework_private_include_directories})
    endif()

    if(${target}_LIBRARIES)
        target_link_libraries(${target} PUBLIC ${${target}_LIBRARIES})
    endif()

    if(${target}_PRIVATE_LIBRARIES)
        target_link_libraries(${target} PRIVATE ${${target}_PRIVATE_LIBRARIES})
    endif()

    if(${target}_DEFINITIONS)
        target_compile_definitions(${target} PUBLIC ${${target}_DEFINITIONS})
    endif()

    if(${target}_PRIVATE_DEFINITIONS)
        target_compile_definitions(${target} PRIVATE ${${target}_PRIVATE_DEFINITIONS})
    endif()

    if(${target}_COMPILE_OPTIONS)
        target_compile_options(${target} PRIVATE ${${target}_COMPILE_OPTIONS})
    endif()

    if(${target}_DEPENDENCIES)
        add_dependencies(${target} ${${target}_DEPENDENCIES})
    endif()
endmacro()
