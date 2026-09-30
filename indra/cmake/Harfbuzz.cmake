  include_guard()

  find_package(harfbuzz CONFIG REQUIRED)

  add_library(ll::harfbuzz INTERFACE IMPORTED)

  target_link_libraries(ll::harfbuzz INTERFACE harfbuzz::harfbuzz)

  find_package(PkgConfig)
  pkg_check_modules(HB-RASTER REQUIRED IMPORTED_TARGET GLOBAL harfbuzz-raster)
  target_link_libraries(ll::harfbuzz INTERFACE PkgConfig::HB-RASTER)

  # Analytic, size-independent glyph rendering. Keep availability tied to the
  # HarfBuzz package itself (llhbgpu.h performs the version gate) so the shared
  # precompiled header never sees a target-specific capability define.
  pkg_check_modules(HB-GPU REQUIRED IMPORTED_TARGET GLOBAL harfbuzz-gpu)
  target_link_libraries(ll::harfbuzz INTERFACE PkgConfig::HB-GPU)
