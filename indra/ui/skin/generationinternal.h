/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include "paint/image.h"
#include "paint/svg.h"
#include "resource/compiler.h"
#include "resource/resourceprovider.h"
#include "skin/generation.h"

namespace radia::ui {
namespace detail {
inline ResourceId resolveSkinResource(const ResourceSnapshot& resources, std::string_view reference) {
    const ResourceId resolved = resources.resolve(ResourceId("skin.css"), reference);
    if (!resolved.valid() || resources.resources().contains(resolved)) return resolved;

    const ResourceId assetReference("resources/" + resolved.value());
    return resources.resources().contains(assetReference) ? assetReference : resolved;
}
} // namespace detail

struct SkinGeneration::Impl {
    Impl(ResourceSnapshot resourcesValue, LocalizationCatalog localizationValue, StyleSheet styleSheetValue,
         std::unordered_map<std::string, SvgImage> svgResourcesValue, std::unordered_map<std::string, RasterImage> rasterResourcesValue = {})
        : resources(std::make_shared<const ResourceSnapshot>(std::move(resourcesValue))), localization(std::move(localizationValue)),
          styleSheet(std::move(styleSheetValue)), svgResources(std::move(svgResourcesValue)), rasterResources(std::move(rasterResourcesValue)),
          resourceCompiler(resources.get()) {}

    std::shared_ptr<const ResourceSnapshot> resources;
    LocalizationCatalog localization;
    StyleSheet styleSheet;
    std::unordered_map<std::string, SvgImage> svgResources;
    std::unordered_map<std::string, RasterImage> rasterResources;
    ResourceCompiler resourceCompiler;
};
} // namespace radia::ui
