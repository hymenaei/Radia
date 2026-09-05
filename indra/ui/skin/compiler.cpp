/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "skin/compiler.h"
#include <unordered_map>
#include "resourceprovider.h"
#include "skin/generation.h"
#include "skin/generationinternal.h"

namespace radia::ui {
namespace {
constexpr const char* kStylesheetId = "skin.css";
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

    const std::optional<ResourceSource> localizationYaml = resources.load(ResourceId("localization.yaml"));
    const std::optional<ResourceSource> styleSource = resources.load(ResourceId(kStylesheetId));
    const std::string defaultStylesheetId(kDefaultStylesheetResourceId);
    if (!localizationYaml) result.error("ui.resource.missing", "Missing UI resource: localization.yaml.", "localization.yaml");
    if (!styleSource) result.error("ui.resource.missing", "Missing UI resource: skin.css.", kStylesheetId);
    if (result.hasErrors()) return result;

    const std::vector<ResourceLayer>& localizationLayers = resources.layers(ResourceId("localization.yaml"));
    const std::vector<ResourceLayer>& styleLayers = resources.layers(ResourceId(kStylesheetId));
    result.append(localizationLayers.empty() ? localization.loadYaml(localizationYaml->content, localizationYaml->provenance)
                                             : localization.loadYamlLayers(localizationLayers));
    std::vector<StyleLayer> styleInputs;
    styleInputs.reserve(styleLayers.empty() ? 2 : styleLayers.size() + 1);
    styleInputs.push_back(StyleLayer{StyleOrigin::Default, ResourceLayer{defaultStylesheetId, std::string(defaultStylesheetSource())}});
    if (styleLayers.empty()) styleInputs.push_back(StyleLayer{StyleOrigin::Skin, ResourceLayer{styleSource->provenance, styleSource->content}});
    else
        for (const ResourceLayer& layer : styleLayers) styleInputs.push_back(StyleLayer{StyleOrigin::Skin, layer});
    result.append(styleSheet.loadRadiaLayers(styleInputs));

    constexpr const char* kResourcePrefix = "resources/";
    constexpr std::size_t kResourcePrefixSize = sizeof("resources/") - 1;
    for (const auto& [id, resource] : resources.resources()) {
        const std::string& path = id.value();
        if (path.rfind(kResourcePrefix, 0) != 0) continue;
        if (path.size() == kResourcePrefixSize) {
            result.error("ui.asset.path_invalid", "Invalid asset resource ID: " + path + ".", resource.provenance);
            continue;
        }

        const std::string assetId = path.substr(kResourcePrefixSize);
        if (assetId.rfind("icons/", 0) != 0 || !endsWith(assetId, ".svg")) {
            result.error("ui.asset.unsupported", "Unsupported UI asset: " + assetId + ".", resource.provenance);
            continue;
        }

        const std::string name = assetId.substr(sizeof("icons/") - 1, assetId.size() - (sizeof("icons/") - 1) - (sizeof(".svg") - 1));
        if (name.empty() || name.front() == '/' || name.find("..") != std::string::npos) {
            result.error("ui.asset.name_invalid", "Invalid icon resource name: " + assetId + ".", resource.provenance);
            continue;
        }

        SvgCompileResult iconResult = compileSvgIcon(resource.content, resource.provenance);
        if (iconResult.ok() && !icons.emplace(name, std::move(*iconResult.icon)).second)
            result.error("ui.icon.duplicate", "Duplicate icon resource: " + name + ".", resource.provenance);
        result.append(std::move(iconResult));
    }
    if (result.hasErrors()) return result;

    for (const auto& entry : resources.resources()) {
        const ResourceId& id = entry.first;
        const ResourceSource& resource = entry.second;
        const std::string& path = id.value();
        if (path == "localization.yaml" || path == kStylesheetId || path.rfind(kResourcePrefix, 0) == 0) continue;
        if (!endsWith(path, kLayoutExtension)) {
            result.error("ui.layout.unsupported", "Unsupported UI layout resource: " + path + ".", resource.provenance);
            continue;
        }
        if (path.rfind("elements/", 0) == 0) {
            const std::string elementName = path.substr(sizeof("elements/") - 1, path.size() - (sizeof("elements/") - 1) - kLayoutExtensionSize);
            if (elementName.empty() || elementName.find('/') != std::string::npos) {
                result.error("ui.layout.defaults_path_invalid", "Element Defaults must use elements/<element>.html: " + path + ".",
                             resource.provenance);
                continue;
            }
        }
    }
    if (result.hasErrors()) return result;

    auto generation = std::shared_ptr<SkinGeneration>(new SkinGeneration(
        std::make_unique<SkinGeneration::Impl>(std::move(resources), std::move(localization), std::move(styleSheet), std::move(icons))));

    for (const auto& entry : generation->mImpl->resources->resources()) {
        const ResourceId& id = entry.first;
        const std::string& path = id.value();
        if (path.rfind(kResourcePrefix, 0) == 0 || path == "localization.yaml" || path == kStylesheetId) continue;
        if (path.rfind("elements/", 0) == 0) {
            const std::string elementName = path.substr(sizeof("elements/") - 1, path.size() - (sizeof("elements/") - 1) - kLayoutExtensionSize);
            result.append(generation->validateElementDefaults(elementName));
        } else result.append(generation->buildElementTree(id, generation->defaultLocale()));
    }
    if (result.hasErrors()) return result;

    result.generation = std::move(generation);
    return result;
}
} // namespace radia::ui
