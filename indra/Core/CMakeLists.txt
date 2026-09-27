set_property(DIRECTORY . PROPERTY FOLDER "Core")

include(Headers.cmake)
include(CoreMacros.cmake)

list(APPEND Core_UNIFIED_SOURCE_LIST_FILES
    "Sources.txt"
)

set(Core_INCLUDE_DIRECTORIES
    "${Core_DERIVED_SOURCES_DIR}"
    "${CORE_DIR}"
    "${CORE_DIR}/css"
    "${CORE_DIR}/css/parser"
    "${CORE_DIR}/css/values"
    "${CORE_DIR}/layout"
    "${CORE_DIR}/style"
)

set(Core_PRIVATE_INCLUDE_DIRECTORIES
    "${CMAKE_BINARY_DIR}"
    "${CORE_DIR}"
    "${CORE_DIR}/dom"
    "${CORE_DIR}/html"
)

set(Core_LIBRARIES
    llcommon
    llmath
    llrender
    llxml
    ll::icu
    ll::yaml-cpp
)

if (WINDOWS)
    list(APPEND Core_SOURCES platform/fonts/windows/WindowsSystemFontProvider.cpp)
    list(APPEND Core_PRIVATE_LIBRARIES dwrite)
elseif (DARWIN)
    list(APPEND Core_SOURCES platform/fonts/mac/MacSystemFontProvider.cpp)
    find_library(CORETEXT_FRAMEWORK CoreText REQUIRED)
    find_library(COREFOUNDATION_FRAMEWORK CoreFoundation REQUIRED)
    list(APPEND Core_PRIVATE_LIBRARIES ${CORETEXT_FRAMEWORK} ${COREFOUNDATION_FRAMEWORK})
elseif (LINUX)
    list(APPEND Core_SOURCES platform/fonts/linux/LinuxSystemFontProvider.cpp)
    list(APPEND Core_PRIVATE_LIBRARIES ll::fontconfig)
endif ()

add_custom_command(
    OUTPUT
    "${Core_DERIVED_SOURCES_DIR}/CSSKeywords.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSKeywords.h"
    "${Core_DERIVED_SOURCES_DIR}/CSSProperties.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSProperties.h"
    "${Core_DERIVED_SOURCES_DIR}/CSSPropertyParsing.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSPropertyParsing.h"
    "${Core_DERIVED_SOURCES_DIR}/CSSPseudoSelectors.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSPseudoSelectors.h"
    "${Core_DERIVED_SOURCES_DIR}/ComputedStyleProperties.cpp"
    "${Core_DERIVED_SOURCES_DIR}/ComputedStyleProperties.h"
    "${Core_DERIVED_SOURCES_DIR}/ComputedStylePropertiesInlines.h"
    "${Core_DERIVED_SOURCES_DIR}/EventTypes.cpp"
    "${Core_DERIVED_SOURCES_DIR}/EventTypes.h"
    "${Core_DERIVED_SOURCES_DIR}/HTMLNames.cpp"
    "${Core_DERIVED_SOURCES_DIR}/HTMLNames.h"
    "${Core_DERIVED_SOURCES_DIR}/InputTypes.cpp"
    "${Core_DERIVED_SOURCES_DIR}/InputTypes.h"
    MAIN_DEPENDENCY "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
    DEPENDS
    "${CORE_DIR}/css/values/CSSKeywords.json5"
    "${CORE_DIR}/css/CSSProperties.json5"
    "${CORE_DIR}/css/CSSPseudoSelectors.json5"
    "${CORE_DIR}/dom/EventTypes.json5"
    "${CORE_DIR}/html/HTMLAttributes.json5"
    "${CORE_DIR}/html/HTMLTags.json5"
    "${CORE_DIR}/html/InputTypes.json5"
    COMMAND "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/Tools/codegen.py" --output "${Core_DERIVED_SOURCES_DIR}"
    VERBATIM
)

add_custom_command(
    OUTPUT "${Core_DERIVED_SOURCES_DIR}/css/UserAgentStyleSheet.cpp"
    MAIN_DEPENDENCY "${CORE_DIR}/css/ua.css"
    DEPENDS "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
    COMMAND "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/Tools/codegen.py"
    --user-agent-stylesheet "${CORE_DIR}/css/ua.css"
    --output "${Core_DERIVED_SOURCES_DIR}/css"
    VERBATIM
)

set(Core_DERIVED_SOURCES
    "${Core_DERIVED_SOURCES_DIR}/CSSKeywords.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSProperties.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSPropertyParsing.cpp"
    "${Core_DERIVED_SOURCES_DIR}/CSSPseudoSelectors.cpp"
    "${Core_DERIVED_SOURCES_DIR}/ComputedStyleProperties.cpp"
    "${Core_DERIVED_SOURCES_DIR}/EventTypes.cpp"
    "${Core_DERIVED_SOURCES_DIR}/HTMLNames.cpp"
    "${Core_DERIVED_SOURCES_DIR}/InputTypes.cpp"
    "${Core_DERIVED_SOURCES_DIR}/css/UserAgentStyleSheet.cpp"
)
list(APPEND Core_SOURCES ${Core_DERIVED_SOURCES})

RADIA_FRAMEWORK_DECLARE(Core)
RADIA_INCLUDE_CONFIG_FILES_IF_EXISTS()
RADIA_COMPUTE_SOURCES(Core)
RADIA_FRAMEWORK(Core)

# Temporary compatibility name if the rest of Radia currently links against ui.
add_library(ui ALIAS Core)

if(BUILD_TESTING)
    add_test(NAME codegen_test COMMAND "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/Tools/codegen_test.py")
    set_tests_properties(codegen_test PROPERTIES LABELS unit)
endif()
