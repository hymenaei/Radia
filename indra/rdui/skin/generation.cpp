/**
 * @file generation.cpp
 * @brief Exposes immutable compiled skin generations for Widget trees, localization, styles, and icons.
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
#include "skin/generation.h"
#include "skin/generationinternal.h"
#include "widgets/icon.h"

namespace rdui {
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
InlineContent SkinGeneration::resolveContent(const std::string& locale, const LocalizationRequest& request) const {
    return mImpl->localization.resolve(locale, request);
}
std::string SkinGeneration::resolveText(const std::string& locale, const LocalizationRequest& request) const {
    return mImpl->localization.get(locale, request);
}
const StyleSheet& SkinGeneration::styleSheet() const {
    return mImpl->styleSheet;
}

std::shared_ptr<const SkinGeneration> SkinGeneration::empty() {
    static const std::shared_ptr<const SkinGeneration> generation(new SkinGeneration(std::make_unique<Impl>(
        ResourceSnapshot(), LocalizationCatalog(), StyleSheet(), std::unordered_map<std::string, SvgIcon>(), LayoutDocumentMap())));
    return generation;
}

LayoutBuildResult SkinGeneration::buildWidgetTree(const std::string& resourceId, const std::string& locale) const {
    const std::string selected_locale = containsLocale(locale) ? locale : defaultLocale();
    const LayoutBuildContext context(mImpl->localization, selected_locale);
    LayoutBuildResult result = mImpl->layoutCompiler.buildWidgetTreeFromResource(resourceId, &context);
    if (result.root) validateIconReferences(*result.root, result);
    if (result.hasErrors()) result.root.reset();
    return result;
}

void SkinGeneration::validateIconReferences(Widget& widget, LayoutBuildResult& result) const {
    if (const auto* icon = dynamic_cast<const Icon*>(&widget); icon && !icon->name().empty() && !this->icon(icon->name()))
        result.error("layout.icon.missing", "Unknown icon resource: " + icon->name() + ".");
    for (const auto& child : widget.children()) validateIconReferences(*child, result);
}

DiagnosticResult SkinGeneration::validateWidgetDefaults(const std::string& element) const {
    const LayoutBuildContext context(mImpl->localization, defaultLocale());
    return mImpl->layoutCompiler.validateWidgetDefaults(element, &context);
}

const SvgIcon* SkinGeneration::icon(const std::string& name) const {
    const auto found = mImpl->icons.find(name);
    return found == mImpl->icons.end() ? nullptr : &found->second;
}
} // namespace rdui
