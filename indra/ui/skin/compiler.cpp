/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "skin/compiler.h"
#include <limits>
#include <unordered_map>
#include "css/syntax.h"
#include "llimage.h"
#include "paint/image.h"
#include "resource/resourceprovider.h"
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

bool isSvgResource(const std::string& value) {
    return endsWith(detail::lower(value), ".svg");
}

std::string imageMimeType(const std::string& value) {
    const std::string lowerValue = detail::lower(value);
    if (endsWith(lowerValue, ".bmp")) return "image/bmp";
    if (endsWith(lowerValue, ".jpg") || endsWith(lowerValue, ".jpeg")) return "image/jpeg";
    if (endsWith(lowerValue, ".png")) return "image/png";
    if (endsWith(lowerValue, ".tga")) return "image/tga";
    if (endsWith(lowerValue, ".webp")) return "image/webp";
    if (endsWith(lowerValue, ".avif")) return "image/avif";
    return {};
}

std::optional<RasterImage> decodeRasterImage(const ResourceSource& source, const std::string& path) {
    const std::string mimeType = imageMimeType(path);
    if (mimeType.empty() || source.content.size() > std::numeric_limits<U32>::max()) return std::nullopt;
    LLPointer<LLImageFormatted> formatted =
        LLImageFormatted::loadFromMemory(reinterpret_cast<const U8*>(source.content.data()), static_cast<U32>(source.content.size()), mimeType);
    if (formatted.isNull()) return std::nullopt;

    LLPointer<LLImageRaw> raw = new LLImageRaw;
    if (!formatted->decode(raw, 100000.f) || raw->getWidth() == 0 || raw->getHeight() == 0 || !raw->getData()) return std::nullopt;
    if (raw->getComponents() < 1 || raw->getComponents() > 4) return std::nullopt;

    const std::size_t pixelCount = static_cast<std::size_t>(raw->getWidth()) * raw->getHeight();
    RasterImage image;
    image.width = raw->getWidth();
    image.height = raw->getHeight();
    image.rgba.resize(pixelCount * 4);
    const U8* sourcePixels = raw->getData();
    const U8 components = raw->getComponents();
    for (std::size_t pixel = 0; pixel < pixelCount; ++pixel) {
        const U8* input = sourcePixels + pixel * components;
        U8* output = image.rgba.data() + pixel * 4;
        if (components == 1 || components == 2) output[0] = output[1] = output[2] = input[0];
        else {
            output[0] = input[0];
            output[1] = input[1];
            output[2] = input[2];
        }
        output[3] = components == 2 || components == 4 ? input[components - 1] : 255;
    }
    return image;
}
} // namespace

SkinGenerationPrepareResult SkinCompiler::prepare(ResourceSnapshot resources) const {
    SkinGenerationPrepareResult result;
    LocalizationCatalog localization;
    StyleSheet styleSheet;
    std::unordered_map<std::string, SvgImage> svgResources;
    std::unordered_map<std::string, RasterImage> rasterResources;

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

    for (const StyleResourceReference& reference : styleSheet.resourceReferences()) {
        const ResourceId resolved = detail::resolveSkinResource(resources, reference.value);
        if (!resolved.valid() || !resources.resources().contains(resolved)) {
            if (reference.optional) continue;
            result.error("ui.resource.missing", "Missing UI resource: " + reference.value + ".", kStylesheetId);
            continue;
        }
        if (reference.cursor) continue;
        if (isSvgResource(resolved.value())) continue;
        if (imageMimeType(resolved.value()).empty()) {
            if (reference.optional)
                result.warning("ui.resource.unsupported", "Optional mask image has an unsupported format: " + reference.value + ".", kStylesheetId);
            else
                result.error("ui.resource.unsupported", "Required image resource has an unsupported format: " + reference.value + ".", kStylesheetId);
            continue;
        }

        const ResourceSource& resource = resources.resources().at(resolved);
        const std::optional<RasterImage> image = decodeRasterImage(resource, resolved.value());
        if (image) {
            rasterResources.emplace(resolved.value(), *image);
        } else if (reference.optional) {
            result.warning("ui.resource.unsupported",
                           "Optional mask image could not be decoded; the element will remain unmasked: " + reference.value + ".", kStylesheetId);
        } else {
            result.error("ui.resource.unsupported", "Required image resource could not be decoded: " + reference.value + ".", kStylesheetId);
        }
    }
    if (result.hasErrors()) return result;

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
        if (!isSvgResource(assetId)) continue;
        SvgCompileResult imageResult = compileSvgImage(resource.content, resource.provenance);
        if (imageResult.ok()) svgResources.emplace(path, std::move(*imageResult.image));
        else {
            bool optional = false;
            for (const StyleResourceReference& reference : styleSheet.resourceReferences()) {
                if (!reference.optional) continue;
                const ResourceId resolved = detail::resolveSkinResource(resources, reference.value);
                if (resolved.value() == path) {
                    optional = true;
                    break;
                }
            }
            if (!optional) result.append(std::move(imageResult));
            else
                for (const Diagnostic& diagnostic : imageResult.errors)
                    result.warning(diagnostic.code, diagnostic.message, diagnostic.source, diagnostic.line, diagnostic.column);
        }
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

    auto generation = std::shared_ptr<SkinGeneration>(new SkinGeneration(std::make_unique<SkinGeneration::Impl>(
        std::move(resources), std::move(localization), std::move(styleSheet), std::move(svgResources), std::move(rasterResources))));

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
