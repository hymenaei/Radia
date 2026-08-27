/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <unordered_map>
#include <utility>
#include "layout/resourcecompiler.h"
#include "render/svg.h"
#include "resourceprovider.h"
#include "skin/generation.h"

namespace radia::ui {
struct SkinGeneration::Impl {
    Impl(ResourceSnapshot resourcesValue, LocalizationCatalog localizationValue, StyleSheet styleSheetValue,
         std::unordered_map<std::string, SvgIcon> iconsValue)
        : resources(std::make_shared<const ResourceSnapshot>(std::move(resourcesValue))), localization(std::move(localizationValue)),
          styleSheet(std::move(styleSheetValue)), icons(std::move(iconsValue)), layoutCompiler(resources.get()) {}

    std::shared_ptr<const ResourceSnapshot> resources;
    LocalizationCatalog localization;
    StyleSheet styleSheet;
    std::unordered_map<std::string, SvgIcon> icons;
    LayoutResourceCompiler layoutCompiler;
};
} // namespace radia::ui
