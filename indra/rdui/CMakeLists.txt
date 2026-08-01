# -*- cmake -*-

add_library(rdui STATIC)

set(rdui_binding_sources binding/binder.cpp)

set(rdui_layout_sources
    layout/document.cpp
    layout/document.h
    layout/engine.cpp
    layout/engine.h
    layout/resourcecompiler.cpp
    layout/resourcecompiler.h
    layout/schema.cpp
    layout/schema.h
    )

set(rdui_localization_sources
    localization/compiler.cpp
    localization/internal.h
    localization/localization.cpp
    localization/richstring.cpp
    localization/yaml.cpp
    )

set(rdui_render_sources
    render/openglpaintcontext.cpp
    render/recordingpaintcontext.cpp
    render/svg.cpp
    render/tessellator.cpp
    )

set(rdui_skin_sources
    skin/compiler.cpp
    skin/generation.cpp
    skin/generationinternal.h
    )

set(rdui_style_sources
    style/color.cpp
    style/compiler.cpp
    style/compiler.h
    style/resolver.cpp
    style/stylesheet.cpp
    style/stylesheetparser.cpp
    style/valueparser.cpp
    )

set(rdui_surface_sources
    surface/floaterresize.cpp
    surface/floaters.cpp
    surface/input.cpp
    surface/resize.cpp
    surface/surface.cpp
    )

set(rdui_text_sources
    text/host.cpp
    text/inlinecontent.cpp
    text/inlinecontentcompiler.cpp
    text/inlinecontentcompiler.h
    text/layout.cpp
    text/layout.h
    text/metrics.cpp
    text/source.cpp
    )

set(rdui_widget_sources
    widgets/button.cpp
    widgets/field.cpp
    widgets/fieldset.cpp
    widgets/floater.cpp
    widgets/icon.cpp
    widgets/label.cpp
    widgets/panel.cpp
    widgets/switch.cpp
    widgets/text.cpp
    widgets/widget.cpp
    widgets/widgetcatalog.cpp
    widgets/widgetcatalog.h
    widgets/widgetcontract.cpp
    widgets/widgetcontract.h
    )

target_sources(rdui
    PRIVATE
    path.cpp
    path.h
    system.cpp
    ${rdui_binding_sources}
    ${rdui_layout_sources}
    ${rdui_localization_sources}
    ${rdui_render_sources}
    ${rdui_skin_sources}
    ${rdui_style_sources}
    ${rdui_surface_sources}
    ${rdui_text_sources}
    ${rdui_widget_sources}

    PUBLIC
    FILE_SET HEADERS
    BASE_DIRS ${CMAKE_CURRENT_SOURCE_DIR}
    FILES
    action.h
    binding/binder.h
    binding/valuebinding.h
    binding/valuecontrol.h
    diagnostic.h
    event.h
    layout/viewresult.h
    localization/localization.h
    localization/value.h
    render/openglpaintcontext.h
    render/paintcontext.h
    render/recordingpaintcontext.h
    resourceprovider.h
    skin/compiler.h
    skin/generation.h
    style/color.h
    style/style.h
    style/stylesheet.h
    surface/floaterresize.h
    surface/surface.h
    system.h
    text/host.h
    text/inlinecontent.h
    text/metrics.h
    text/source.h
    types.h
    widgets/button.h
    widgets/field.h
    widgets/fieldset.h
    widgets/floater.h
    widgets/icon.h
    widgets/label.h
    widgets/panel.h
    widgets/switch.h
    widgets/text.h
    widgets/widget.h
    )

target_include_directories(rdui INTERFACE $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}>)

target_link_libraries(rdui
        PUBLIC
        llcommon
        llmath
        llrender
        llxml
        ll::icu
        ll::yaml-cpp
        )

target_precompile_headers(rdui REUSE_FROM llprecompiled)

if (BUILD_TESTING)
  function(rdui_add_test logical_name source_path)
    set(target PROJECT_rdui_TEST_${logical_name})
    add_executable(${target} ${source_path})
    target_link_libraries(${target} PRIVATE rdui lltut_runner_lib ll::tut ll::expat)
    target_include_directories(${target} PRIVATE ${INDRA_SOURCE_DIR}/test)
    target_compile_definitions(${target} PRIVATE "LL_TEST=${logical_name}" "LL_TEST_${logical_name}")
    set_target_properties(${target} PROPERTIES
        RUNTIME_OUTPUT_DIRECTORY "${EXE_STAGING_DIR}"
        FOLDER "Tests/rdui")
    if (WINDOWS)
      target_link_options(${target} PRIVATE $<$<CONFIG:Release>:/DEBUG:NONE>)
    endif ()
    set(test_output ${CMAKE_CURRENT_BINARY_DIR}/${target}_ok.txt)
    set(test_command $<TARGET_FILE:${target}> --touch=${test_output} --sourcedir=${CMAKE_CURRENT_SOURCE_DIR})
    SET_TEST_PATH(LD_LIBRARY_PATH)
    LL_TEST_COMMAND(test_script "${LD_LIBRARY_PATH}" ${test_command})
    add_test(NAME ${target} COMMAND ${test_script} WORKING_DIRECTORY ${CMAKE_CURRENT_BINARY_DIR})
    add_dependencies(BUILD_TESTS ${target})
  endfunction()

  rdui_add_test(rduibinder tests/binding/binder_test.cpp)
  rdui_add_test(rduicolor tests/style/color_test.cpp)
  rdui_add_test(rduicomponentcontract tests/layout/componentcontract_test.cpp)
  rdui_add_test(rduifloaterresize tests/surface/floaterresize_test.cpp)
  rdui_add_test(rduisurface tests/surface/surface_test.cpp)
  rdui_add_test(rduisurfacefloater tests/surface/floaters_test.cpp)
  rdui_add_test(rduilayout tests/layout/engine_test.cpp)
  rdui_add_test(rduilocalization tests/localization/localization_test.cpp)
  rdui_add_test(rduistyle tests/style/style_test.cpp)
  rdui_add_test(rduistyleproperty tests/style/property_test.cpp)
  rdui_add_test(rduisystem tests/system_test.cpp)
  rdui_add_test(rduisvg tests/render/svg_test.cpp)
  rdui_add_test(rduivaluebinding tests/binding/valuebinding_test.cpp)
  rdui_add_test(rduiwidget tests/widgets/widget_test.cpp)
  rdui_add_test(rduitessellator tests/render/tessellator_test.cpp)
  rdui_add_test(rduilayoutresourcecompiler tests/layout/resourcecompiler_test.cpp)
  rdui_add_test(rduidiagnostic tests/diagnostic_test.cpp)
  rdui_add_test(rduipath tests/path_test.cpp)
endif ()
