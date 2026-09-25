include_guard()

find_package(ICU REQUIRED COMPONENTS i18n uc)

add_library(ll::icu INTERFACE IMPORTED)
target_link_libraries(ll::icu INTERFACE ICU::i18n ICU::uc)
