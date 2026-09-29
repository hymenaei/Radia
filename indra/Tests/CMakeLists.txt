set(TestCore_SOURCES
    Core/Binder.cpp
    Core/BorderImageGrid.cpp
    Core/Color.cpp
    Core/Diagnostic.cpp
    Core/Element.cpp
    Core/EventHandlerCall.cpp
    Core/Fieldset.cpp
    Core/FloaterResize.cpp
    Core/Floaters.cpp
    Core/GeneratedStyle.cpp
    Core/Input.cpp
    Core/LayoutEngine.cpp
    Core/Localization.cpp
    Core/PaintProtocol.cpp
    Core/Path.cpp
    Core/ResourceCompiler.cpp
    Core/ScrollGeometry.cpp
    Core/SkinCompiler.cpp
    Core/StyleSheet.cpp
    Core/Surface.cpp
    Core/SurfaceState.cpp
    Core/SVG.cpp
    Core/System.cpp
    Core/SystemFontProvider.cpp
    Core/Tessellator.cpp
)
set(TestCore_PRIVATE_LIBRARIES Core GTest::gtest_main)
set(TestCore_PRIVATE_DEFINITIONS "RADIA_SOURCE_ROOT=\"${CMAKE_SOURCE_DIR}\"")
RADIA_TEST(TestCore)

set(TestViewer_SOURCES
    Viewer/ComponentManager.cpp
    Viewer/FloaterHost.cpp
    Viewer/InputBridge.cpp
    Viewer/ReloadCoordinator.cpp
    Viewer/Resolver.cpp
    Viewer/Runtime.cpp
    Viewer/SettingsAdapter.cpp
    Viewer/SkinPreparation.cpp
    Viewer/WorkspacePersistence.cpp
)
set(TestViewer_PRIVATE_LIBRARIES Viewer GTest::gtest_main)
set(TestViewer_PRIVATE_DEFINITIONS "RADIA_SOURCE_ROOT=\"${CMAKE_SOURCE_DIR}\"")
RADIA_TEST(TestViewer)

if(BUILD_TESTING)
    add_test(NAME codegen_test COMMAND "${Python3_EXECUTABLE}" "${CMAKE_SOURCE_DIR}/Tools/codegen_test.py")
    set_tests_properties(codegen_test PROPERTIES LABELS unit)

    if(BUILD_VIEWER)
        RADIA_EXECUTABLE_DECLARE(llui_libtest)
        set(llui_libtest_SOURCES
            llui_libtest.cpp
            llui_libtest.h
            llwidgetreg.cpp
            llwidgetreg.h
        )
        set(llui_libtest_PRIVATE_DEFINITIONS "LLUI_LIBTEST_NEWVIEW_PATH=\"${INDRA_SOURCE_DIR}/newview\"")
        set(llui_libtest_PRIVATE_LIBRARIES
            llui
            llinventory
            llmessage
            llrender
            llimage
            llimagej2coj
        )

        if(USE_KDU)
            list(APPEND llui_libtest_PRIVATE_LIBRARIES llkdu)
        endif()

        RADIA_EXECUTABLE(llui_libtest)
        set_target_properties(llui_libtest PROPERTIES FOLDER "Tests/Integration")

        add_dependencies(${VIEWER_BINARY_NAME} llui_libtest)

        if(TARGET BUILD_TESTS)
            add_dependencies(BUILD_TESTS llui_libtest)
        endif()

        LL_TEST_LIBRARY_PATH(llui_libtest_library_path)
        LL_TEST_COMMAND(llui_libtest_command
            "${llui_libtest_library_path}"
            "$<TARGET_FILE:llui_libtest>"
        )
        add_test(NAME llui_libtest COMMAND ${llui_libtest_command})
        set_tests_properties(llui_libtest
            PROPERTIES
            LABELS integration
            WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
        )
    endif()

    RADIA_EXECUTABLE_DECLARE(llimage_libtest)
    set(llimage_libtest_SOURCES
        llimage_libtest.cpp
        llimage_libtest.h
    )
    set(llimage_libtest_PRIVATE_LIBRARIES
        llcommon
        llfilesystem
        llmath
        llimage
    )
    RADIA_EXECUTABLE(llimage_libtest)
    set_target_properties(llimage_libtest PROPERTIES FOLDER "Tests/Integration")

    if(BUILD_VIEWER)
        add_dependencies(${VIEWER_BINARY_NAME} llimage_libtest)
    endif()

    if(TARGET BUILD_TESTS)
        add_dependencies(BUILD_TESTS llimage_libtest)
    endif()

    LL_TEST_LIBRARY_PATH(llimage_libtest_library_path)
    LL_TEST_COMMAND(llimage_libtest_command
        "${llimage_libtest_library_path}"
        "$<TARGET_FILE:llimage_libtest>"
        "--help"
    )
    add_test(NAME llimage_libtest COMMAND ${llimage_libtest_command})
    set_tests_properties(llimage_libtest
        PROPERTIES
        LABELS integration
        WORKING_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}"
    )
endif()

set(BenchmarkCore_SOURCES
    Core/Benchmarks/LayoutEngine.cpp
)
set(BenchmarkCore_PRIVATE_LIBRARIES Core benchmark::benchmark_main)
RADIA_BENCHMARK(BenchmarkCore)
