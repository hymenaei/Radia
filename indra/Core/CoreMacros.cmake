include(RadiaMacros)
include(RadiaFS)

macro(Core_CODEGEN)
    set(_core_compiled_sources)
    set(_core_metadata_outputs)
    set(_core_generated_headers)

    foreach(_core_source IN LISTS Core_DERIVED_SOURCES)
        file(RELATIVE_PATH _core_relative_source "${Core_DERIVED_SOURCES_DIR}" "${_core_source}")

        if(NOT IS_ABSOLUTE "${_core_source}" OR _core_relative_source MATCHES "^\\.\\."
            OR NOT _core_relative_source MATCHES "^[A-Za-z0-9_/-]+\\.(cpp|h)$")
            message(FATAL_ERROR "Core_DERIVED_SOURCES entries must be .cpp or .h paths under Core_DERIVED_SOURCES_DIR: ${_core_source}")
        endif()

        if(_core_relative_source MATCHES "\\.cpp$")
            list(APPEND _core_compiled_sources "${_core_source}")
            list(APPEND _core_metadata_outputs "${_core_source}")
        else()
            list(APPEND _core_generated_headers "${_core_source}")
        endif()
    endforeach()

    set(_core_stylesheet_output "${Core_DERIVED_SOURCES_DIR}/css/UserAgentStyleSheet.cpp")

    if(Core_USERAGENTSTYLESHEET_SOURCES)
        list(LENGTH Core_USERAGENTSTYLESHEET_SOURCES _core_stylesheet_count)

        if(NOT _core_stylesheet_count EQUAL 1)
            message(FATAL_ERROR "Core supports one user-agent stylesheet source")
        endif()

        if(NOT _core_stylesheet_output IN_LIST _core_compiled_sources)
            message(FATAL_ERROR "Core_DERIVED_SOURCES must include ${_core_stylesheet_output}")
        endif()

        list(REMOVE_ITEM _core_metadata_outputs "${_core_stylesheet_output}")
    endif()

    add_custom_command(
        OUTPUT ${_core_metadata_outputs} ${_core_generated_headers}
        MAIN_DEPENDENCY "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
        DEPENDS ${Core_METADATA_SOURCES}
        COMMAND "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
        --output "${Core_DERIVED_SOURCES_DIR}"
        VERBATIM
    )

    foreach(_core_stylesheet IN LISTS Core_USERAGENTSTYLESHEET_SOURCES)
        add_custom_command(
            OUTPUT "${_core_stylesheet_output}"
            MAIN_DEPENDENCY "${_core_stylesheet}"
            DEPENDS "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
            COMMAND "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
            --user-agent-stylesheet "${_core_stylesheet}"
            --output "${Core_DERIVED_SOURCES_DIR}/css"
            VERBATIM
        )
    endforeach()

    list(APPEND Core_SOURCES ${_core_compiled_sources})
endmacro()
