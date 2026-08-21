/**
 * @file compiler.cpp
 * @brief Compiles layered skin resources into immutable runtime generations.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include "skin/compiler.h"
#include <unordered_map>
#include "layout/document.h"
#include "skin/generation.h"
#include "skin/generationinternal.h"

namespace radia::ui {
namespace {
bool endsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}
} // namespace

SkinGenerationPrepareResult SkinCompiler::prepare(ResourceSnapshot resources) const {
    SkinGenerationPrepareResult result;
    LocalizationCatalog localization;
    StyleSheet styleSheet;
    std::unordered_map<std::string, SvgIcon> icons;
    LayoutDocumentMap layoutDocuments;

    const std::optional<std::string> localizationYaml = resources.load("localization.yaml");
    const std::optional<std::string> styleSource = resources.load("skin.radia");
    if (!localizationYaml) result.error("rdui.resource.missing", "Missing Radia UI resource: localization.yaml.", "localization.yaml");
    if (!styleSource) result.error("rdui.resource.missing", "Missing Radia UI resource: skin.radia.", "skin.radia");
    if (result.hasErrors()) return result;

    const std::vector<ResourceLayer>& localizationLayers = resources.layers("localization.yaml");
    const std::vector<ResourceLayer>& styleLayers = resources.layers("skin.radia");
    result.append(localizationLayers.empty() ? localization.loadYaml(*localizationYaml, "localization.yaml")
                                              : localization.loadYamlLayers(localizationLayers));
    result.append(styleLayers.empty() ? styleSheet.loadRadia(*styleSource, "skin.radia") : styleSheet.loadRadiaLayers(styleLayers));

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
        const std::string& sourceText = resource.second;
        if (resourceId == "localization.yaml" || resourceId == "skin.radia" || resourceId.rfind(kResourcePrefix, 0) == 0) continue;
        if (!endsWith(resourceId, ".xml")) {
            result.error("rdui.layout.unsupported", "Unsupported Radia UI layout resource: " + resourceId + ".", resourceId);
            continue;
        }
        if (resourceId.rfind("widgets/", 0) == 0) {
            const std::string element =
                resourceId.substr(sizeof("widgets/") - 1, resourceId.size() - (sizeof("widgets/") - 1) - (sizeof(".xml") - 1));
            if (element.empty() || element.find('/') != std::string::npos) {
                result.error("rdui.layout.defaults_path_invalid", "Widget Defaults must use widgets/<element>.xml: " + resourceId + ".",
                             resourceId);
                continue;
            }
        }

        LayoutDocumentParseResult parsed = LayoutDocumentParser().parse(sourceText, resourceId);
        std::unique_ptr<LayoutDocument> document = std::move(parsed.document);
        result.append(std::move(parsed));
        if (document) layoutDocuments.emplace(resourceId, std::shared_ptr<const LayoutDocument>(std::move(document)));
    }
    if (result.hasErrors()) return result;

    auto generation = std::shared_ptr<SkinGeneration>(new SkinGeneration(std::make_unique<SkinGeneration::Impl>(
        std::move(resources), std::move(localization), std::move(styleSheet), std::move(icons), std::move(layoutDocuments))));

    for (const auto& resource : generation->mImpl->resources->resources()) {
        const std::string& resourceId = resource.first;
        if (generation->mImpl->layoutDocuments.find(resourceId) == generation->mImpl->layoutDocuments.end()) continue;
        if (resourceId.rfind("widgets/", 0) == 0) {
            const std::string element =
                resourceId.substr(sizeof("widgets/") - 1, resourceId.size() - (sizeof("widgets/") - 1) - (sizeof(".xml") - 1));
            result.append(generation->validateWidgetDefaults(element));
        } else result.append(generation->buildWidgetTree(resourceId, generation->defaultLocale()));
    }
    if (result.hasErrors()) return result;

    result.generation = std::move(generation);
    return result;
}
} // namespace radia::ui
