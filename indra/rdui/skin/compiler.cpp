/**
 * @file compiler.cpp
 * @brief
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

namespace rdui {
namespace {
bool endsWith(const std::string& value, const std::string& suffix) {
    return value.size() >= suffix.size() && value.compare(value.size() - suffix.size(), suffix.size(), suffix) == 0;
}
} // namespace

SkinGenerationPrepareResult SkinCompiler::prepare(ResourceSnapshot resources) const {
    SkinGenerationPrepareResult result;
    LocalizationCatalog localization;
    StyleSheet style_sheet;
    std::unordered_map<std::string, SvgIcon> icons;
    LayoutDocumentMap layout_documents;

    const std::optional<std::string> localization_yaml = resources.load("localization.yaml");
    const std::optional<std::string> style_source = resources.load("skin.radia");
    if (!localization_yaml) result.error("rdui.resource.missing", "Missing Radia UI resource: localization.yaml.", "localization.yaml");
    if (!style_source) result.error("rdui.resource.missing", "Missing Radia UI resource: skin.radia.", "skin.radia");
    if (result.hasErrors()) return result;

    const std::vector<ResourceLayer>& localization_layers = resources.layers("localization.yaml");
    const std::vector<ResourceLayer>& style_layers = resources.layers("skin.radia");
    result.append(localization_layers.empty() ? localization.loadYaml(*localization_yaml, "localization.yaml")
                                              : localization.loadYamlLayers(localization_layers));
    result.append(style_layers.empty() ? style_sheet.loadRadia(*style_source, "skin.radia") : style_sheet.loadRadiaLayers(style_layers));

    constexpr const char* RESOURCE_PREFIX = "resources/";
    constexpr std::size_t RESOURCE_PREFIX_SIZE = sizeof("resources/") - 1;
    for (const auto& [resource_id, source_text] : resources.resources()) {
        if (resource_id.rfind(RESOURCE_PREFIX, 0) != 0) continue;
        if (resource_id.size() == RESOURCE_PREFIX_SIZE) {
            result.error("rdui.asset.path_invalid", "Invalid asset resource ID: " + resource_id + ".", resource_id);
            continue;
        }

        const std::string asset_id = resource_id.substr(RESOURCE_PREFIX_SIZE);
        if (asset_id.rfind("icons/", 0) != 0 || !endsWith(asset_id, ".svg")) {
            result.error("rdui.asset.unsupported", "Unsupported Radia UI asset: " + asset_id + ".", resource_id);
            continue;
        }

        const std::string name = asset_id.substr(sizeof("icons/") - 1, asset_id.size() - (sizeof("icons/") - 1) - (sizeof(".svg") - 1));
        if (name.empty() || name.front() == '/' || name.find("..") != std::string::npos) {
            result.error("rdui.asset.name_invalid", "Invalid icon resource name: " + asset_id + ".", resource_id);
            continue;
        }

        SvgCompileResult icon_result = compileSvgIcon(source_text, resource_id);
        if (icon_result.ok() && !icons.emplace(name, std::move(*icon_result.icon)).second)
            result.error("rdui.icon.duplicate", "Duplicate icon resource: " + name + ".", resource_id);
        result.append(std::move(icon_result));
    }
    if (result.hasErrors()) return result;

    for (const auto& resource : resources.resources()) {
        const std::string& resource_id = resource.first;
        const std::string& source_text = resource.second;
        if (resource_id == "localization.yaml" || resource_id == "skin.radia" || resource_id.rfind(RESOURCE_PREFIX, 0) == 0) continue;
        if (!endsWith(resource_id, ".xml")) {
            result.error("rdui.layout.unsupported", "Unsupported Radia UI layout resource: " + resource_id + ".", resource_id);
            continue;
        }
        if (resource_id.rfind("widgets/", 0) == 0) {
            const std::string element =
                resource_id.substr(sizeof("widgets/") - 1, resource_id.size() - (sizeof("widgets/") - 1) - (sizeof(".xml") - 1));
            if (element.empty() || element.find('/') != std::string::npos) {
                result.error("rdui.layout.defaults_path_invalid", "Widget Defaults must use widgets/<element>.xml: " + resource_id + ".",
                             resource_id);
                continue;
            }
        }

        LayoutDocumentParseResult parsed = LayoutDocumentParser().parse(source_text, resource_id);
        result.append(std::move(parsed));
        if (parsed.document) layout_documents.emplace(resource_id, std::shared_ptr<const LayoutDocument>(std::move(parsed.document)));
    }
    if (result.hasErrors()) return result;

    auto generation = std::shared_ptr<SkinGeneration>(new SkinGeneration(std::make_unique<SkinGeneration::Impl>(
        std::move(resources), std::move(localization), std::move(style_sheet), std::move(icons), std::move(layout_documents))));

    for (const auto& resource : generation->mImpl->resources->resources()) {
        const std::string& resource_id = resource.first;
        if (generation->mImpl->layout_documents.find(resource_id) == generation->mImpl->layout_documents.end()) continue;
        if (resource_id.rfind("widgets/", 0) == 0) {
            const std::string element =
                resource_id.substr(sizeof("widgets/") - 1, resource_id.size() - (sizeof("widgets/") - 1) - (sizeof(".xml") - 1));
            result.append(generation->validateWidgetDefaults(element));
        } else result.append(generation->createView(resource_id, generation->defaultLocale()));
    }
    if (result.hasErrors()) return result;

    result.generation = std::move(generation);
    return result;
}
} // namespace rdui
