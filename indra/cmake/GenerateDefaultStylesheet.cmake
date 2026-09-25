# -*- cmake -*-

include_guard(GLOBAL)

function(GENERATE_DEFAULT_STYLESHEET)
    cmake_parse_arguments(PARSE_ARGV 0 stylesheet "" "SOURCE;OUTPUT" "")

    if(stylesheet_UNPARSED_ARGUMENTS
        OR NOT stylesheet_SOURCE
        OR NOT stylesheet_OUTPUT)
        message(FATAL_ERROR "GENERATE_DEFAULT_STYLESHEET requires SOURCE and OUTPUT.")
    endif()

    file(READ "${stylesheet_SOURCE}" stylesheet_text)

    string(FIND "${stylesheet_text}" [=[)__RADIA__"]=] terminator_position)

    if(NOT terminator_position EQUAL -1)
        message(FATAL_ERROR "The default stylesheet contains the generated raw-string terminator.")
    endif()

    get_filename_component(output_directory "${stylesheet_OUTPUT}" DIRECTORY)
    file(MAKE_DIRECTORY "${output_directory}")

    set(source_prefix [=[/**
 * Generated from defaults.css. Do not edit.
 */

#include <string_view>

namespace radia::ui {
std::string_view defaultStylesheetSource() noexcept {
    return R"__RADIA__(]=])
    set(source_suffix [=[)__RADIA__";
}
} // namespace radia::ui
]=])

    file(GENERATE
        OUTPUT "${stylesheet_OUTPUT}"
        CONTENT "${source_prefix}${stylesheet_text}${source_suffix}"
        NEWLINE_STYLE LF
    )
    set_property(
        DIRECTORY APPEND
        PROPERTY CMAKE_CONFIGURE_DEPENDS "${stylesheet_SOURCE}"
    )
endfunction()
