/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "skin/generation.h"
#include "elements/elementdefinition.h"
#include "elements/icon.h"
#include "skin/generationinternal.h"

namespace radia::ui {
SkinGeneration::SkinGeneration(std::unique_ptr<Impl> implementation) : mImpl(std::move(implementation)) {}
SkinGeneration::~SkinGeneration() = default;

std::vector<LocaleInfo> SkinGeneration::locales() const {
    return mImpl->localization.locales();
}
const std::string& SkinGeneration::defaultLocale() const {
    return mImpl->localization.defaultLocaleId();
}
const LocaleInfo* SkinGeneration::locale(const std::string& id) const {
    return mImpl->localization.locale(id);
}
bool SkinGeneration::containsLocale(const std::string& id) const {
    return mImpl->localization.containsLocale(id);
}
bool SkinGeneration::hasLocalizationKey(const std::string& id) const {
    return mImpl->localization.containsDefaultString(id);
}
std::string SkinGeneration::resolveMarkup(const std::string& locale, const LocalizedText& text) const {
    return mImpl->localization.resolveMarkup(locale, text);
}
std::string SkinGeneration::resolveText(const std::string& locale, const LocalizedText& text) const {
    return mImpl->localization.resolveText(locale, text);
}
const StyleSheet& SkinGeneration::styleSheet() const {
    return mImpl->styleSheet;
}

std::shared_ptr<const SkinGeneration> SkinGeneration::empty() {
    StyleSheet styleSheet;
    (void)styleSheet.loadRadia(std::string(defaultStylesheetSource()), std::string(kDefaultStylesheetResourceId));
    return std::shared_ptr<const SkinGeneration>(new SkinGeneration(
        std::make_unique<Impl>(ResourceSnapshot(), LocalizationCatalog(), std::move(styleSheet), std::unordered_map<std::string, SvgIcon>())));
}

LayoutBuildResult SkinGeneration::buildElementTree(const std::string& resourceId, const std::string& locale) const {
    const std::string selectedLocale = containsLocale(locale) ? locale : defaultLocale();
    const LayoutBuildContext context(mImpl->localization, selectedLocale);
    LayoutBuildResult result = mImpl->layoutCompiler.buildElementTreeFromResource(resourceId, &context);
    if (result.document && result.document->documentElement()) validateIconReferences(*result.document->documentElement(), result);
    if (result.hasErrors()) result.document.reset();
    return result;
}

void SkinGeneration::validateIconReferences(Element& element, LayoutBuildResult& result) const {
    if (const auto* icon = dynamic_cast<const IconElement*>(&element); icon && !icon->name().empty() && !this->icon(icon->name()))
        result.error("layout.icon.missing", "Unknown icon resource: " + icon->name() + ".");
    for (Element* child : element.children()) validateIconReferences(*child, result);
}

DiagnosticResult SkinGeneration::validateElementDefaults(const std::string& element) const {
    const LayoutBuildContext context(mImpl->localization, defaultLocale());
    return mImpl->layoutCompiler.validateElementDefaults(element, &context);
}

const SvgIcon* SkinGeneration::icon(const std::string& name) const {
    const auto found = mImpl->icons.find(name);
    return found == mImpl->icons.end() ? nullptr : &found->second;
}
} // namespace radia::ui
