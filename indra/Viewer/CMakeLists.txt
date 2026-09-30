set_property(DIRECTORY . PROPERTY FOLDER "Viewer")

include(Headers.cmake)
include(ViewerMacros.cmake)

list(APPEND Viewer_SOURCE_LIST_FILES "Sources.txt")

set(Viewer_PRIVATE_INCLUDE_DIRECTORIES
    "${VIEWER_DIR}"
    "${INDRA_SOURCE_DIR}/newview"
    "${INDRA_BINARY_DIR}/newview"
)

set(Viewer_LIBRARIES
    Core
    llcommon
    llfilesystem
    llrender
    llwindow
)

RADIA_FRAMEWORK_DECLARE(Viewer)
RADIA_INCLUDE_CONFIG_FILES_IF_EXISTS()
RADIA_COMPUTE_SOURCES(Viewer)
RADIA_FRAMEWORK(Viewer)
