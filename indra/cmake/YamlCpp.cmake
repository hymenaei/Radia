include_guard()

find_package(yaml-cpp CONFIG REQUIRED)

add_library(ll::yaml-cpp INTERFACE IMPORTED)
target_link_libraries(ll::yaml-cpp INTERFACE yaml-cpp::yaml-cpp)
