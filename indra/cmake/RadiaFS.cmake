if(NOT CORE_DIR)
    set(CORE_DIR "${CMAKE_SOURCE_DIR}/Core")
endif()

if(NOT VIEWER_DIR)
    set(VIEWER_DIR "${CMAKE_SOURCE_DIR}/Viewer")
endif()

if(NOT TESTS_DIR)
    set(TESTS_DIR "${CMAKE_SOURCE_DIR}/Tests")
endif()

set(Core_DERIVED_SOURCES_DIR "${CMAKE_BINARY_DIR}/Core/DerivedSources")

set(Viewer_DERIVED_SOURCES_DIR "${CMAKE_BINARY_DIR}/Viewer/DerivedSources")
