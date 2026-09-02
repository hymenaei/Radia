# -*- cmake -*-

add_library(ui STATIC)

include(GenerateDefaultStylesheet)

set(default_stylesheet_source "${CMAKE_CURRENT_SOURCE_DIR}/style/defaults.css")
set(default_stylesheet_output "${CMAKE_CURRENT_BINARY_DIR}/style/defaults.inc")
generate_default_stylesheet(SOURCE "${default_stylesheet_source}" OUTPUT "${default_stylesheet_output}")

set(private_sources
    eventcall.cpp
    eventcallinternal.h
    localization.cpp
    nativeappearance.cpp
    path.cpp
    system.cpp

    binding/binder.cpp
    binding/settingresolver.h

    dom/document.cpp
    dom/element.cpp
    dom/fragment.cpp
    dom/mutation.cpp
    dom/mutation.h
    dom/node.cpp
    dom/text.cpp
    resource/elementdefinition.cpp
    resource/elementdefinition.h
    dom/elementinternal.h

    html/button.cpp
    html/fieldset.cpp
    html/fieldset.h
    html/floater.cpp
    html/fragment.cpp
    html/fragmentinternal.h
    html/icon.cpp
    html/input.cpp
    html/inputcheckbox.cpp
    html/inputradio.cpp
    html/label.cpp
    html/panel.cpp

    html/elementfactory.cpp
    html/elementfactory.h
    html/elementnames.cpp
    html/elementnames.h
    html/element.cpp

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

    html/button.h
    html/floater.h
    html/icon.h
    html/input.h
    dom/fragment.h
    dom/document.h
    dom/node.h
    style/pseudoelement.h
    html/label.h
    html/panel.h
    html/element.h
    dom/element.h
    dom/text.h
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

if(BUILD_TESTING OR BUILD_BENCHMARKING)
    add_subdirectory(tests)
endif()
