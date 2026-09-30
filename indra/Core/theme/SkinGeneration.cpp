/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "SkinGeneration.h"
#include "ResourceElementDefinition.h"
#include "ResourceProvider.h"
#include "SkinGenerationInternal.h"
#include "StyleSheet.h"
#include "UserAgentStyleSheet.h"

namespace Core {
SkinGeneration::SkinGeneration(std::unique_ptr<Impl> implementation)
    : mImpl(std::move(implementation)) {}
SkinGeneration::~SkinGeneration() = default;

std::vector<LocaleInfo> SkinGeneration::locales() const { return mImpl->localization.locales(); }
const std::string& SkinGeneration::defaultLocale() const { return mImpl->localization.defaultLocaleId(); }
const LocaleInfo* SkinGeneration::locale(const std::string& id) const { return mImpl->localization.locale(id); }
bool SkinGeneration::containsLocale(const std::string& id) const { return mImpl->localization.containsLocale(id); }
bool SkinGeneration::hasLocalizationKey(const std::string& id) const { return mImpl->localization.containsDefaultString(id); }
std::string SkinGeneration::resolveHTML(const std::string& locale, const LocalizedText& text) const {
    return mImpl->localization.resolveHTML(locale, text);
}
std::string SkinGeneration::resolveText(const std::string& locale, const LocalizedText& text) const {
    return mImpl->localization.resolveText(locale, text);
}
const CSS::StyleSheet& SkinGeneration::styleSheet() const { return mImpl->styleSheet; }
const std::vector<CSS::FontFace>& SkinGeneration::fontFaces() const { return mImpl->styleSheet.fontFaces(); }

std::shared_ptr<const SkinGeneration> SkinGeneration::empty() {
    CSS::StyleSheet styleSheet;
    (void)styleSheet.loadRadia(std::string(CSS::userAgentStyleSheet()), std::string(CSS::kUserAgentStyleSheetId));
    return std::shared_ptr<const SkinGeneration>(new SkinGeneration(std::make_unique<Impl>(ResourceSnapshot(), LocalizationCatalog(),
        std::move(styleSheet), std::unordered_map<std::string, SVGImage>())));
}

ResourceBuildResult SkinGeneration::buildElementTree(const ResourceId& id, const std::string& locale) const {
    const std::string selectedLocale = containsLocale(locale) ? locale : defaultLocale();
    const ResourceBuildContext context(mImpl->localization, selectedLocale);
    ResourceBuildResult result = mImpl->resourceCompiler.buildElementTreeFromResource(id, &context);
    if (result.hasErrors())
        result.document.reset();
    return result;
}

DiagnosticResult SkinGeneration::validateElementDefaults(const std::string& elementName) const {
    const ResourceBuildContext context(mImpl->localization, defaultLocale());
    return mImpl->resourceCompiler.validateElementDefaults(elementName, &context);
}

const SVGImage* SkinGeneration::resourceSvg(std::string_view reference) const {
    const ResourceId resolved = detail::resolveSkinResource(*mImpl->resources, reference);
    const auto found = mImpl->svgResources.find(resolved.value());
    return found == mImpl->svgResources.end() ? nullptr : &found->second;
}

const RasterImage* SkinGeneration::resourceRaster(std::string_view reference) const {
    const ResourceId resolved = detail::resolveSkinResource(*mImpl->resources, reference);
    const auto found = mImpl->rasterResources.find(resolved.value());
    return found == mImpl->rasterResources.end() ? nullptr : &found->second;
}

const std::string* SkinGeneration::resourceData(std::string_view reference) const {
    const ResourceId resolved = detail::resolveSkinResource(*mImpl->resources, reference);
    const auto found = mImpl->resources->resources().find(resolved);
    return found == mImpl->resources->resources().end() ? nullptr : &found->second.content;
}
} // namespace Core
