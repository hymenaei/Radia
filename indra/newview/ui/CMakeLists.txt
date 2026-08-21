# -*- cmake -*-

add_library(viewer_ui STATIC)

set(viewer_ui_skin_sources
  skin/reloadcoordinator.cpp
  skin/reloadcoordinator.h
  skin/resolver.cpp
  skin/resolver.h
  skin/resources.cpp
  skin/resources.h
)

set(viewer_ui_components_sources
  components/floaterdemo.cpp
  components/floaterdemo.h
)

target_sources(viewer_ui
  PRIVATE
  componentcontroller.cpp
  componentcontroller.h
  componentcontrollereventadapter.h
  componentcontrollerevents.h
  componentcontrollerregistration.h
  componentidentity.h
  componentmanager.cpp
  componentmanager.h
  componentpersistence.cpp
  componentpersistence.h
  floaterhost.cpp
  floaterhost.h
  inputbridge.cpp
  inputbridge.h
  nativeinput.h
  runtime.cpp
  runtime.h
  settingsadapter.cpp
  settingsadapter.h
  ${viewer_ui_skin_sources}
  ${viewer_ui_components_sources}
)

target_include_directories(viewer_ui
  PUBLIC ${CMAKE_CURRENT_SOURCE_DIR}
  PRIVATE ${CMAKE_CURRENT_SOURCE_DIR}/..
)

target_link_libraries(viewer_ui
  PUBLIC
  llcommon
  llmath
  llfilesystem
  llmessage
  llcharacter
  llprimitive
  llrender
  llwindow
  llxml
  ui
)

target_precompile_headers(viewer_ui REUSE_FROM llprecompiled)

if(BUILD_TESTING)
  set(viewer_ui_test_sources
    componentmanager.cpp
    componentpersistence.cpp
    inputbridge.cpp
    runtime.cpp
    settingsadapter.cpp
    skin/reloadcoordinator.cpp
    skin/resolver.cpp
  )

  rd_add_unit_tests(viewer_ui "${viewer_ui_test_sources}")
endif()
