# -*- cmake -*-

if(BUILD_TESTING)
    add_executable(ui_test
        binder_test.cpp
        color_test.cpp
        compiler_test.cpp
        element_test.cpp
        engine_test.cpp
        eventcall_test.cpp
        fieldset_test.cpp
        floaterresize_test.cpp
        floaters_test.cpp
        input_test.cpp
        localization_test.cpp
        paintprotocol_test.cpp
        path_test.cpp
        resourcecompiler_test.cpp
        diagnostic_test.cpp
        scrollgeometry_test.cpp
        stylesheet_test.cpp
        surface_test.cpp
        surfacestate_test.cpp
        svg_test.cpp
        system_test.cpp
        tessellator_test.cpp
    )

    target_include_directories(ui_test PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/..")
    target_link_libraries(ui_test
        PRIVATE
        ui
        GTest::gtest_main
    )
    _CONFIGURE_GTEST_TARGET(ui_test unit)
endif()

if(BUILD_BENCHMARKING)
    add_executable(ui_benchmarks
        engine_benchmark.cpp
    )
    target_include_directories(ui_benchmarks PRIVATE "${CMAKE_CURRENT_SOURCE_DIR}/..")
    target_link_libraries(ui_benchmarks
        PRIVATE
        ui
        benchmark::benchmark_main
    )
    _CONFIGURE_BENCHMARK_TARGET(ui_benchmarks)
endif()
