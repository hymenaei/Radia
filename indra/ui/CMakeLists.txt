# -*- cmake -*-

add_library(ui STATIC)

include(GenerateDefaultStylesheet)

set(default_stylesheet_source "${CMAKE_CURRENT_SOURCE_DIR}/style/defaults.css")
set(default_stylesheet_output "${CMAKE_CURRENT_BINARY_DIR}/style/defaults.inc")
generate_default_stylesheet(SOURCE "${default_stylesheet_source}" OUTPUT "${default_stylesheet_output}")

set(private_sources
    eventcall.cpp
    localization.cpp
    nativeappearance.cpp
    path.cpp
    system.cpp

    binding/binder.cpp
    binding/settingresolver.h

    elements/button.cpp
    elements/document.cpp
    elements/element.cpp
    elements/elementtext.cpp
    elements/elementdefinition.cpp
    elements/elementdefinition.h
    elements/elementinternal.h
    elements/fieldset.cpp
    elements/floater.cpp
    elements/icon.cpp
    elements/input.cpp
    elements/inputcheckbox.cpp
    elements/inputradio.cpp
    elements/label.cpp
    elements/panel.cpp

    layout/arrange.cpp
    layout/document.cpp
    layout/document.h
    layout/engine.cpp
    layout/engine.h
    layout/engineinternal.h
    layout/layoutcontext.h
    layout/measure.cpp
    layout/primitives.cpp
    layout/primitives.h
    layout/resourcecompiler.cpp
    layout/resourcecompiler.h
    layout/schema.cpp
    layout/schema.h
    layout/treecache.cpp
    layout/treecache.h

    render/openglpaintcontext.cpp
    render/openglpaintstate.cpp
    render/openglpaintstate.h
    render/paintprotocol.def
    render/recordingpaintcontext.cpp
    render/svg.cpp
    render/tessellator.cpp

    skin/compiler.cpp
    skin/generation.cpp
    skin/generationinternal.h

    style/color.cpp
    style/compiler.cpp
    style/defaults.css
    style/model.h
    style/resolver.cpp
    style/stylepass.cpp
    style/stylepass.h
    style/stylesheet.cpp
    style/stylesheetparser.cpp
    style/syntax.cpp
    style/syntax.h
    style/valueparser.cpp

    surface/floaterresize.cpp
    surface/floaters.cpp
    surface/input.cpp
    surface/resize.cpp
    surface/scrollgeometry.cpp
    surface/surface.cpp
    surface/surfaceinternal.h

    text/host.cpp
    text/inlineelements.cpp
    text/inlineelements.h
    text/layout.cpp
    text/layout.h
    text/metrics.cpp
)

set(public_sources
    diagnostic.h
    event.h
    eventcall.h
    localization.h
    localizedtext.h
    nativeappearance.h
    path.h
    resourceprovider.h
    system.h
    types.h

    binding/valuebinding.h
    binding/eventregistration.h

    layout/buildresult.h

    render/openglpaintcontext.h
    render/paintcontext.h
    render/painttarget.h
    render/recordingpaintcontext.h

    skin/compiler.h
    skin/generation.h

    style/color.h
    style/style.h
    style/stylesheet.h

    surface/floaterresize.h
    surface/scrollgeometry.h
    surface/surface.h

    text/keybinding.h
    text/metrics.h

    elements/elementevent.h
    elements/button.h
    elements/floater.h
    elements/icon.h
    elements/input.h
    elements/document.h
    elements/label.h
    elements/panel.h
    elements/element.h
    elements/elementtext.h
)

target_sources(ui
    PRIVATE ${private_sources}
    PUBLIC
    FILE_SET HEADERS
    FILES ${public_sources}
)

target_include_directories(ui PRIVATE "${CMAKE_CURRENT_BINARY_DIR}")

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

if(BUILD_TESTING)
    set(test_sources
        eventcall.cpp
        localization.cpp
        path.cpp
        system.cpp

        binding/binder.cpp

        elements/element.cpp
        elements/fieldset.cpp
        elements/inputcheckbox.cpp

        layout/engine.cpp
        layout/resourcecompiler.cpp
        layout/schema.cpp

        render/svg.cpp
        render/tessellator.cpp

        style/color.cpp
        style/compiler.cpp
        style/stylesheet.cpp

        surface/floaterresize.cpp
        surface/floaters.cpp
        surface/scrollgeometry.cpp
        surface/surface.cpp
    )

    ADD_UNIT_TESTS(ui "${test_sources}")
    ADD_INTEGRATION_TEST(surfacestate "" "ui" "ui")
endif()

if(BUILD_BENCHMARKING)
    ADD_BENCHMARKS(ui "layout/engine.cpp")
endif()
