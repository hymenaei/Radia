set_property(DIRECTORY . PROPERTY FOLDER "Core")

include(Headers.cmake)
include(CoreMacros.cmake)

list(APPEND Core_SOURCE_LIST_FILES "Sources.txt")

set(Core_INCLUDE_DIRECTORIES
    "${Core_DERIVED_SOURCES_DIR}"
)

set(Core_PRIVATE_INCLUDE_DIRECTORIES
    "${CMAKE_BINARY_DIR}"
    "${CORE_DIR}/bindings"
    "${CORE_DIR}/css"
    "${CORE_DIR}/css/parser"
    "${CORE_DIR}/css/values"
    "${CORE_DIR}/dom"
    "${CORE_DIR}/html"
    "${CORE_DIR}/inspector"
    "${CORE_DIR}/layout"
    "${CORE_DIR}/loader"
    "${CORE_DIR}/localization"
    "${CORE_DIR}/platform/graphics"
    "${CORE_DIR}/rendering"
    "${CORE_DIR}/runtime"
    "${CORE_DIR}/style"
    "${CORE_DIR}/style/computed"
    "${CORE_DIR}/surface"
    "${CORE_DIR}/svg/graphics"
    "${CORE_DIR}/theme"
)

set(Core_LIBRARIES
    llcommon
    llmath
    llrender
    llxml
    ll::icu
    ll::yaml-cpp
)

set(Core_METADATA_SOURCES
    "${CORE_DIR}/css/CSSProperties.json5"
    "${CORE_DIR}/css/CSSPseudoSelectors.json5"
    "${CORE_DIR}/css/values/CSSKeywords.json5"
    "${CORE_DIR}/dom/EventTypes.json5"
    "${CORE_DIR}/html/HTMLAttributes.json5"
    "${CORE_DIR}/html/HTMLTags.json5"
    "${CORE_DIR}/html/InputTypes.json5"
)

set(Core_USERAGENTSTYLESHEET_SOURCES
    "${CORE_DIR}/css/ua.css"
)

set(Core_DERIVED_SOURCES
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
    "${Core_DERIVED_SOURCES_DIR}/css/UserAgentStyleSheet.cpp"
)

RADIA_FRAMEWORK_DECLARE(Core)
RADIA_INCLUDE_CONFIG_FILES_IF_EXISTS()
RADIA_COMPUTE_SOURCES(Core)
RADIA_FRAMEWORK(Core)
