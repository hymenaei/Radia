# -*- cmake -*-

add_library(rdui STATIC)

set(rdui_event_sources
    binding/eventdescriptor.h
    binding/eventregistration.h
    eventcall.cpp
    widgetevents.def
    )

set(rdui_binding_sources
    binding/binder.cpp
    binding/settingresolver.h
    )

set(rdui_layout_sources
    layout/document.cpp
    layout/document.h
    layout/engine.cpp
    layout/layoutcontext.h
    layout/engine.h
    layout/measure.cpp
    layout/arrange.cpp
    layout/treecache.cpp
    layout/treecache.h
    layout/primitives.cpp
    layout/primitives.h
    layout/engineinternal.h
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
    render/openglpaintstate.cpp
    render/openglpaintstate.h
    render/paintprotocol.def
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
    style/model.h
    style/resolver.cpp
    style/stylepass.cpp
    style/stylepass.h
    style/stylesheet.cpp
    style/stylesheetparser.cpp
    style/syntax.cpp
    style/syntax.h
    style/valueparser.cpp
    )

set(rdui_surface_sources
    surface/floaterresize.cpp
    surface/floaters.cpp
    surface/input.cpp
    surface/resize.cpp
    surface/surface.cpp
    surface/ordering.h
    )

set(rdui_text_sources
    text/host.cpp
    text/inlinecontent.cpp
    text/inlinecontent.def
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
    widgets/widgetcontractbuilder.h
    widgets/widgetcontractmodel.h
    )

target_sources(rdui
    PRIVATE
    path.cpp
    path.h
    system.cpp
    ${rdui_event_sources}
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
    binding/valuebinding.h
    binding/valuecontrol.h
    binding/eventdescriptor.h
    diagnostic.h
    event.h
    eventcall.h
    layout/buildresult.h
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
    text/inlinecontent.def
    text/inlinecontent.h
    text/metrics.h
    text/source.h
    types.h
    widgetevent.h
    widgetevents.def
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
  set(rdui_test_sources
    binding/binder.cpp
    eventcall.cpp
    widgets/switch.cpp
    style/color.cpp
    widgets/field.cpp
    surface/floaterresize.cpp
    surface/surface.cpp
    surface/floaters.cpp
    layout/engine.cpp
    layout/schema.cpp
    localization/localization.cpp
    style/stylesheet.cpp
    style/compiler.cpp
    system.cpp
    render/svg.cpp
    widgets/widget.cpp
    render/tessellator.cpp
    layout/resourcecompiler.cpp
    path.cpp
  )

  set_source_files_properties(
    ${rdui_test_sources}
    PROPERTIES
    LL_TEST_ADDITIONAL_PROJECTS rdui
  )

  LL_ADD_PROJECT_UNIT_TESTS(rdui "${rdui_test_sources}")
  LL_ADD_INTEGRATION_TEST(surfacestate "" "rdui" "rdui")
endif ()
