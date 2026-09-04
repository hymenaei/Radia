# -*- cmake -*-

add_executable(newview_test
    componentmanager_test.cpp
    floaterhost_test.cpp
    inputbridge_test.cpp
    reloadcoordinator_test.cpp
    resolver_test.cpp
    runtime_test.cpp
    settingsadapter_test.cpp
    skinpreparation_test.cpp
    workspacepersistence_test.cpp

    ../componentmanager.cpp
    ../componentmanagerreplacement.cpp
    ../floaterhost.cpp
    ../inputbridge.cpp
    ../reloadcoordinator.cpp
    ../resolver.cpp
    ../runtime.cpp
    ../settingsadapter.cpp
    ../skinpreparation.cpp
    ../workspacepersistence.cpp
    ../documentcontroller.cpp
    ../resources.cpp
)

target_include_directories(newview_test
    PRIVATE
    "${CMAKE_CURRENT_BINARY_DIR}/.."
    "${CMAKE_CURRENT_SOURCE_DIR}/.."
)
target_compile_definitions(newview_test PRIVATE RADIA_SOURCE_ROOT="${CMAKE_SOURCE_DIR}")

target_link_libraries(newview_test
    PRIVATE
    GTest::gtest_main
    llcommon
    llmath
    llfilesystem
    llmessage
    llcharacter
    llprimitive
    llrender
    llwindow
    llxml
    ui
)
_CONFIGURE_GTEST_TARGET(newview_test unit)
