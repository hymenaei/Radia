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
#include "RasterImage.h"
#include "ResourceCompiler.h"
#include "ResourceProvider.h"
#include "SVGImage.h"
#include "SkinGeneration.h"

namespace Core {
namespace detail {
inline ResourceId resolveSkinResource(const ResourceSnapshot& resources, std::string_view reference) {
    const ResourceId resolved = resources.resolve(ResourceId("skin.css"), reference);
    if (!resolved.valid() || resources.resources().contains(resolved))
        return resolved;

    const ResourceId assetReference("resources/" + resolved.value());
    return resources.resources().contains(assetReference) ? assetReference : resolved;
}
} // namespace detail

struct SkinGeneration::Impl {
    Impl(ResourceSnapshot resourcesValue, LocalizationCatalog localizationValue, CSS::StyleSheet styleSheetValue,
        std::unordered_map<std::string, SVGImage> svgResourcesValue, std::unordered_map<std::string, RasterImage> rasterResourcesValue = {})
        : resources(std::make_shared<const ResourceSnapshot>(std::move(resourcesValue)))
        , localization(std::move(localizationValue))
        , styleSheet(std::move(styleSheetValue))
        , svgResources(std::move(svgResourcesValue))
        , rasterResources(std::move(rasterResourcesValue))
        , resourceCompiler(resources.get()) {}

    std::shared_ptr<const ResourceSnapshot> resources;
    LocalizationCatalog localization;
    CSS::StyleSheet styleSheet;
    std::unordered_map<std::string, SVGImage> svgResources;
    std::unordered_map<std::string, RasterImage> rasterResources;
    ResourceCompiler resourceCompiler;
};
} // namespace Core
