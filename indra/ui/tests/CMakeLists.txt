# -*- cmake -*-

if(BUILD_TESTING)
    add_executable(ui_test
        binder_test.cpp
        color_test.cpp
        compiler_test.cpp
        element_test.cpp
        eventcall_test.cpp
        fieldset_test.cpp
        floaterresize_test.cpp
        floaters_test.cpp
        inputcheckbox_test.cpp
        layout_engine_test.cpp
        layout_resourcecompiler_test.cpp
        layout_schema_test.cpp
        localization_test.cpp
        path_test.cpp
        scrollgeometry_test.cpp
        stylesheet_test.cpp
        surface_test.cpp
        svg_test.cpp
        system_test.cpp
        tessellator_test.cpp
    )

    target_include_directories(ui_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/..")
    target_link_libraries(ui_test
        PRIVATE
        ui
        GTest::gtest_main
        GTest::gmock
    )
    _CONFIGURE_GTEST_TARGET(ui_test unit)

    add_executable(ui_surfacestate_test
        surfacestate_test.cpp
    )
    target_include_directories(ui_surfacestate_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/..")
    target_link_libraries(ui_surfacestate_test
        PRIVATE
        ui
        GTest::gtest_main
        GTest::gmock
    )
    _CONFIGURE_GTEST_TARGET(ui_surfacestate_test integration)
endif()

if(BUILD_BENCHMARKING)
    add_executable(ui_benchmarks
        layout_engine_benchmark.cpp
    )
    target_include_directories(ui_benchmarks PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/..")
    target_link_libraries(ui_benchmarks
        PRIVATE
        ui
        benchmark::benchmark_main
    )
    _CONFIGURE_BENCHMARK_TARGET(ui_benchmarks)
endif()
