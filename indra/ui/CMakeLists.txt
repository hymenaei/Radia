# -*- cmake -*-

add_library(ui STATIC)

set(ui_event_sources
    binding/eventdescriptor.h
    binding/eventregistration.h
    eventcall.cpp
    widgetevents.def
    )

set(ui_binding_sources
    binding/binder.cpp
    binding/settingresolver.h
    )

set(ui_layout_sources
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

set(ui_localization_sources
    localization/compiler.cpp
    localization/internal.h
    localization/localization.cpp
    localization/richstring.cpp
    localization/yaml.cpp
    )

set(ui_render_sources
    render/openglpaintcontext.cpp
    render/openglpaintstate.cpp
    render/openglpaintstate.h
    render/paintprotocol.def
    render/recordingpaintcontext.cpp
    render/svg.cpp
    render/tessellator.cpp
    )

set(ui_skin_sources
    skin/compiler.cpp
    skin/generation.cpp
    skin/generationinternal.h
    )

set(ui_style_sources
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

set(ui_surface_sources
    surface/floaterresize.cpp
    surface/floaters.cpp
    surface/input.cpp
    surface/resize.cpp
    surface/surface.cpp
    surface/ordering.h
    )

set(ui_text_sources
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

set(ui_widget_sources
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

target_sources(ui
    PRIVATE
    path.cpp
    path.h
    system.cpp
    ${ui_event_sources}
    ${ui_binding_sources}
    ${ui_layout_sources}
    ${ui_localization_sources}
    ${ui_render_sources}
    ${ui_skin_sources}
    ${ui_style_sources}
    ${ui_surface_sources}
    ${ui_text_sources}
    ${ui_widget_sources}

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

target_include_directories(ui INTERFACE $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}>)

target_link_libraries(ui
        PUBLIC
        llcommon
        llmath
        llrender
        llxml
        ll::icu
        ll::yaml-cpp
        )

target_precompile_headers(ui REUSE_FROM llprecompiled)

if (BUILD_TESTING)
  set(ui_test_sources
    localization/localization.cpp
    binding/binder.cpp
    eventcall.cpp
    path.cpp
    widgets/switch.cpp
    widgets/field.cpp
    style/color.cpp
    style/compiler.cpp
    style/stylesheet.cpp
    system.cpp
    render/svg.cpp
    render/tessellator.cpp
    surface/floaters.cpp
    surface/floaterresize.cpp
    surface/surface.cpp
    layout/engine.cpp
    layout/schema.cpp
    layout/resourcecompiler.cpp
    widgets/widget.cpp
  )

  RD_ADD_UNIT_TESTS(ui "${ui_test_sources}")
  RD_ADD_INTEGRATION_TEST(surfacestate "" "ui" "ui")
endif ()

if (BUILD_BENCHMARKS)
  RD_ADD_BENCHMARKS(ui "layout/engine.cpp")
endif ()
