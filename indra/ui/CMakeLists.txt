# -*- cmake -*-

add_library(ui STATIC)

include(GenerateDefaultStylesheet)

set(default_stylesheet_source "${CMAKE_CURRENT_SOURCE_DIR}/css/defaults.css")
set(default_stylesheet_output "${CMAKE_CURRENT_BINARY_DIR}/css/defaults.inc")
generate_default_stylesheet(SOURCE "${default_stylesheet_source}" OUTPUT "${default_stylesheet_output}")

set(private_sources
    localization.cpp
    system.cpp

    binding/binder.cpp
    binding/settingresolver.h

    css/color.cpp
    css/cssparser.cpp
    css/defaults.css
    css/rules.h
    css/stylesheet.cpp
    css/syntax.cpp
    css/syntax.h
    css/valueparser.cpp

    dom/document.cpp
    dom/element.cpp
    dom/elementinternal.h
    dom/fragment.cpp
    dom/fragmentinternal.h
    dom/fragmentparser.cpp
    dom/mutation.cpp
    dom/mutation.h
    dom/node.cpp
    dom/text.cpp

    event/eventcall.cpp
    event/eventcallinternal.h

    html/button.cpp
    html/element.cpp
    html/elementfactory.cpp
    html/elementfactory.h
    html/elementnames.cpp
    html/elementnames.h
    html/fieldset.cpp
    html/fieldset.h
    html/floater.cpp
    html/input.cpp
    html/inputcheckbox.cpp
    html/inputradio.cpp
    html/label.cpp
    html/panel.cpp

    layout/arrange.cpp
    layout/engine.cpp
    layout/engine.h
    layout/layoutcontext.h
    layout/measure.cpp
    layout/primitives.cpp
    layout/primitives.h
    layout/treecache.cpp
    layout/treecache.h

    paint/nativeappearance.cpp
    paint/image.h
    paint/openglpaintcontext.cpp
    paint/openglpaintstate.cpp
    paint/openglpaintstate.h
    paint/paintprotocol.def
    paint/path.cpp
    paint/recordingpaintcontext.cpp
    paint/svg.cpp
    paint/tessellator.cpp

    resource/compiler.cpp
    resource/compiler.h
    resource/elementdefinition.cpp
    resource/elementdefinition.h
    resource/sourcedocument.cpp
    resource/sourcedocument.h

    skin/compiler.cpp
    skin/generation.cpp
    skin/generationinternal.h

    style/property.cpp
    style/property.h
    style/resolver.cpp
    style/stylepass.cpp
    style/stylepass.h

    surface/floaterresize.cpp
    surface/floaters.cpp
    surface/resize.cpp
    surface/scrollgeometry.cpp
    surface/surfacehost.cpp
    surface/surfacelayout.cpp
    surface/surfacepaint.cpp
    surface/surfaceinput.cpp
    surface/surfacescroll.cpp

    text/inlineelements.cpp
    text/inlineelements.h
    text/layout.cpp
    text/layout.h
    text/metrics.cpp
)

set(public_sources
    diagnostic.h
    localization.h
    localizedtext.h
    system.h
    types.h

    binding/eventregistration.h
    binding/valuebinding.h

    css/color.h
    css/stylesheet.h

    dom/document.h
    dom/element.h
    dom/fragment.h
    dom/node.h
    dom/text.h

    event/event.h
    event/eventcall.h

    html/button.h
    html/element.h
    html/floater.h
    html/input.h
    html/label.h
    html/panel.h

    paint/nativeappearance.h
    paint/openglpaintcontext.h
    paint/paintcontext.h
    paint/painttarget.h
    paint/path.h
    paint/recordingpaintcontext.h

    resource/buildresult.h
    resource/resourceprovider.h

    skin/compiler.h
    skin/generation.h

    style/computedstyle.h
    style/pseudoelement.h

    surface/floaterresize.h
    surface/scrollgeometry.h
    surface/surface.h

    text/keybinding.h
    text/metrics.h
)

target_sources(ui
    PRIVATE ${private_sources}
    PUBLIC
    FILE_SET HEADERS
    FILES ${public_sources}
)

target_include_directories(ui
    PUBLIC "${CMAKE_CURRENT_SOURCE_DIR}"
    PRIVATE "${CMAKE_CURRENT_BINARY_DIR}")

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
