include_guard(GLOBAL)

macro(RADIA_FRAMEWORK_DECLARE target)
    add_library(${target} STATIC)
endmacro()

macro(RADIA_INCLUDE_CONFIG_FILES_IF_EXISTS)
    include("${CMAKE_CURRENT_SOURCE_DIR}/Platform${PORT}.cmake" OPTIONAL)
endmacro()

function(RADIA_COMPUTE_SOURCES target)
    set(sources ${${target}_SOURCES})

    foreach(source_list_file IN LISTS ${target}_UNIFIED_SOURCE_LIST_FILES)
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

macro(RADIA_FRAMEWORK target)
    target_sources(${target} PRIVATE ${${target}_SOURCES} ${${target}_HEADERS})

    if(${target}_INCLUDE_DIRECTORIES)
        target_include_directories(${target} PUBLIC ${${target}_INCLUDE_DIRECTORIES})
    endif()

    if(${target}_PRIVATE_INCLUDE_DIRECTORIES)
        target_include_directories(${target} PRIVATE ${${target}_PRIVATE_INCLUDE_DIRECTORIES})
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
