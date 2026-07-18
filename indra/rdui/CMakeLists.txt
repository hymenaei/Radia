# -*- cmake -*-

add_library(rdui STATIC)

target_sources(rdui
    PRIVATE
    rdbutton.cpp
    rdfloater.cpp
    rduifloaterresize.cpp
    rduifloaterresize.h
    rdfield.cpp
    rdicon.cpp
    rdlabel.cpp
    rdpanel.cpp
    rdswitch.cpp
    rduibinder.cpp
    rduicolor.cpp
    rduisurface.cpp
    rduisurfaceresize.cpp
    rduilayout.cpp
    rduilayout.h
    rduilayoutdocument.cpp
    rduilayoutdocument.h
    rduilayoutresourcecompiler.cpp
    rduilayoutresourcecompiler.h
    rduilocalization.cpp
    rduiwidget.cpp
    rduipath.cpp
    rduiopenglpaintcontext.cpp
    rduirecordingpaintcontext.cpp
    rduiskincompiler.cpp
    rduiskingeneration.cpp
    rduiskingenerationinternal.h
    rduistylecompiler.cpp
    rduistylecompiler.h
    rduistyleresolver.cpp
    rduistylesheet.cpp
    rduistylesheetparser.cpp
    rduisystem.cpp
    rduitext.cpp
    rduisvg.cpp
    rduitessellator.cpp
    rduiviewcontract.cpp
    rduiviewcontract.h

    PUBLIC
    FILE_SET HEADERS
    FILES
    rdbutton.h
    rdfloater.h
    rdfield.h
    rdicon.h
    rdlabel.h
    rdpanel.h
    rdswitch.h
    rduiaction.h
    rduibinder.h
    rduicolor.h
    rduisurface.h
    rduidiagnostic.h
    rduievent.h
    rduilocalization.h
    rduipaintcontext.h
    rduiwidget.h
    rduiresourceprovider.h
    rduiskincompiler.h
    rduiskingeneration.h
    rduiopenglpaintcontext.h
    rduirecordingpaintcontext.h
    rduistyle.h
    rduistylesheet.h
    rduisystem.h
    rduitext.h
    rduitypes.h
    rduiviewresult.h
    )

target_include_directories(rdui INTERFACE ${CMAKE_CURRENT_SOURCE_DIR})

target_link_libraries(rdui
        PUBLIC
        llcommon
        llmath
        llrender
        llxml
        )

target_precompile_headers(rdui REUSE_FROM llprecompiled)

if (BUILD_TESTING)
  function(rdui_add_test name)
    set(target PROJECT_rdui_TEST_${name})
    add_executable(${target} tests/${name}_test.cpp)
    target_link_libraries(${target} PRIVATE rdui lltut_runner_lib ll::tut ll::expat)
    target_include_directories(${target} PRIVATE ${INDRA_SOURCE_DIR}/test)
    target_compile_definitions(${target} PRIVATE "LL_TEST=${name}" "LL_TEST_${name}")
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

  foreach(test_name rduibinder rduicolor rduifloaterresize rduisurface rduipath rduilayout rduilocalization rduistyle rduisystem rduisvg rduiwidget rduitessellator rduilayoutresourcecompiler)
    rdui_add_test(${test_name})
  endforeach()
endif ()
