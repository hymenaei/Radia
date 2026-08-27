/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "skin/compiler.h"
#include <unordered_map>
#include "skin/generation.h"
#include "skin/generationinternal.h"

namespace radia::ui {
namespace {
constexpr const char* kStylesheetResourceId = "skin.css";
constexpr const char* kLayoutExtension = ".html";
constexpr std::size_t kLayoutExtensionSize = sizeof(".html") - 1;

bool endsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}
} // namespace

SkinGenerationPrepareResult SkinCompiler::prepare(ResourceSnapshot resources) const {
    SkinGenerationPrepareResult result;
    LocalizationCatalog localization;
    StyleSheet styleSheet;
    std::unordered_map<std::string, SvgIcon> icons;

    const std::optional<std::string> localizationYaml = resources.load("localization.yaml");
    const std::optional<std::string> styleSource = resources.load(kStylesheetResourceId);
    const std::string defaultStylesheetResourceId(kDefaultStylesheetResourceId);
    if (!localizationYaml) result.error("rdui.resource.missing", "Missing Radia UI resource: localization.yaml.", "localization.yaml");
    if (!styleSource) result.error("rdui.resource.missing", "Missing Radia UI resource: skin.css.", kStylesheetResourceId);
    if (result.hasErrors()) return result;

    const std::vector<ResourceLayer>& localizationLayers = resources.layers("localization.yaml");
    const std::vector<ResourceLayer>& styleLayers = resources.layers(kStylesheetResourceId);
    result.append(localizationLayers.empty() ? localization.loadYaml(*localizationYaml, "localization.yaml")
                                             : localization.loadYamlLayers(localizationLayers));
    std::vector<StyleLayer> styleInputs;
    styleInputs.reserve(styleLayers.empty() ? 2 : styleLayers.size() + 1);
    styleInputs.push_back(StyleLayer{StyleOrigin::Default, ResourceLayer{defaultStylesheetResourceId, std::string(defaultStylesheetSource())}});
    if (styleLayers.empty()) styleInputs.push_back(StyleLayer{StyleOrigin::Skin, ResourceLayer{kStylesheetResourceId, *styleSource}});
    else
        for (const ResourceLayer& layer : styleLayers) styleInputs.push_back(StyleLayer{StyleOrigin::Skin, layer});
    result.append(styleSheet.loadRadiaLayers(styleInputs));

    constexpr const char* kResourcePrefix = "resources/";
    constexpr std::size_t kResourcePrefixSize = sizeof("resources/") - 1;
    for (const auto& [resourceId, sourceText] : resources.resources()) {
        if (resourceId.rfind(kResourcePrefix, 0) != 0) continue;
        if (resourceId.size() == kResourcePrefixSize) {
            result.error("rdui.asset.path_invalid", "Invalid asset resource ID: " + resourceId + ".", resourceId);
            continue;
        }

        const std::string assetId = resourceId.substr(kResourcePrefixSize);
        if (assetId.rfind("icons/", 0) != 0 || !endsWith(assetId, ".svg")) {
            result.error("rdui.asset.unsupported", "Unsupported Radia UI asset: " + assetId + ".", resourceId);
            continue;
        }

        const std::string name = assetId.substr(sizeof("icons/") - 1, assetId.size() - (sizeof("icons/") - 1) - (sizeof(".svg") - 1));
        if (name.empty() || name.front() == '/' || name.find("..") != std::string::npos) {
            result.error("rdui.asset.name_invalid", "Invalid icon resource name: " + assetId + ".", resourceId);
            continue;
        }

        SvgCompileResult icon_result = compileSvgIcon(sourceText, resourceId);
        if (icon_result.ok() && !icons.emplace(name, std::move(*icon_result.icon)).second)
            result.error("rdui.icon.duplicate", "Duplicate icon resource: " + name + ".", resourceId);
        result.append(std::move(icon_result));
    }
    if (result.hasErrors()) return result;

    for (const auto& resource : resources.resources()) {
        const std::string& resourceId = resource.first;
        if (resourceId == "localization.yaml" || resourceId == kStylesheetResourceId || resourceId.rfind(kResourcePrefix, 0) == 0) continue;
        if (!endsWith(resourceId, kLayoutExtension)) {
            result.error("rdui.layout.unsupported", "Unsupported Radia UI layout resource: " + resourceId + ".", resourceId);
            continue;
        }
        if (resourceId.rfind("elements/", 0) == 0) {
            const std::string element =
                resourceId.substr(sizeof("elements/") - 1, resourceId.size() - (sizeof("elements/") - 1) - kLayoutExtensionSize);
            if (element.empty() || element.find('/') != std::string::npos) {
                result.error("rdui.layout.defaults_path_invalid", "Element Defaults must use elements/<element>.html: " + resourceId + ".",
                             resourceId);
                continue;
            }
        }
    }
    if (result.hasErrors()) return result;

    auto generation = std::shared_ptr<SkinGeneration>(new SkinGeneration(
        std::make_unique<SkinGeneration::Impl>(std::move(resources), std::move(localization), std::move(styleSheet), std::move(icons))));

    for (const auto& resource : generation->mImpl->resources->resources()) {
        const std::string& resourceId = resource.first;
        if (resourceId.rfind(kResourcePrefix, 0) == 0 || resourceId == "localization.yaml" || resourceId == kStylesheetResourceId) continue;
        if (resourceId.rfind("elements/", 0) == 0) {
            const std::string element =
                resourceId.substr(sizeof("elements/") - 1, resourceId.size() - (sizeof("elements/") - 1) - kLayoutExtensionSize);
            result.append(generation->validateElementDefaults(element));
        } else result.append(generation->buildElementTree(resourceId, generation->defaultLocale()));
    }
    if (result.hasErrors()) return result;

    result.generation = std::move(generation);
    return result;
}
} // namespace radia::ui
