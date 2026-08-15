/**
 * @file system.cpp
 * @brief Owns UI skin generations, locale state, surfaces, and viewer-facing services.
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
#include "system.h"
#include "skin/generation.h"
#include "surface/surface.h"

namespace radia::ui {
System::System() : mSkinGeneration(SkinGeneration::empty()) {}

System::~System() = default;

bool System::publish(std::shared_ptr<const SkinGeneration> generation) {
    return publishImpl(std::move(generation), nullptr);
}

bool System::publish(std::shared_ptr<const SkinGeneration> generation, PublicationCommit& commit) {
    return publishImpl(std::move(generation), &commit);
}

bool System::publishImpl(std::shared_ptr<const SkinGeneration> generation, PublicationCommit* commit) {
    if (!generation) return false;

    const std::shared_ptr<const SkinGeneration> previousGeneration = mSkinGeneration;
    const std::string previousLocale = mActiveLocale;
    const std::uint64_t previousGenerationNumber = mGenerationNumber;
    const std::uint64_t previousLocaleGeneration = mLocaleGeneration;

    mSkinGeneration = std::move(generation);
    mActiveLocale = mSkinGeneration->containsLocale(previousLocale) ? previousLocale : mSkinGeneration->defaultLocale();
    ++mGenerationNumber;
    ++mLocaleGeneration;

    for (Surface* surface : mSurfaces)
        if (surface) surface->generationChanged(styleSheet());

    if (commit && !commit->commit()) {
        const bool localeChanged = mActiveLocale != previousLocale;
        mSkinGeneration = previousGeneration;
        mActiveLocale = previousLocale;
        mGenerationNumber = previousGenerationNumber;
        mLocaleGeneration = previousLocaleGeneration;
        for (Surface* surface : mSurfaces)
            if (surface) surface->generationChanged(styleSheet());
        if (localeChanged) notifyLocaleChanged();
        return false;
    }

    notifyLocaleChanged();
    return true;
}

std::vector<LocaleInfo> System::locales() const {
    return mSkinGeneration->locales();
}

const std::string& System::defaultLocale() const {
    return mSkinGeneration->defaultLocale();
}

const LocaleInfo* System::activeLocaleInfo() const {
    return mSkinGeneration->locale(mActiveLocale);
}

bool System::hasLocalizationKey(const std::string& id) const {
    return mSkinGeneration->hasLocalizationKey(id);
}

std::string System::resolveText(const std::string& id) const {
    return resolveText(LocalizationRequest::text(id));
}

std::string System::resolveText(const LocalizationRequest& request) const {
    return mSkinGeneration->resolveText(mActiveLocale, request);
}

InlineContent System::resolveContent(const LocalizationRequest& request) const {
    return mSkinGeneration->resolveContent(mActiveLocale, request);
}

TextSource System::localize(std::string id) const {
    return localize(LocalizationRequest::text(std::move(id)));
}

TextSource System::localize(LocalizationRequest request) const {
    InlineContent content = resolveContent(request);
    return TextSource::fromLocalization(std::move(request), std::move(content));
}

const StyleSheet& System::styleSheet() const {
    return mSkinGeneration->styleSheet();
}

const SvgIcon* System::icon(const std::string& name) const {
    return mSkinGeneration->icon(name);
}

bool System::hasIcon(const std::string& name) const {
    return icon(name) != nullptr;
}

bool System::setLocale(const std::string& localeId) {
    if (!mSkinGeneration->containsLocale(localeId)) return false;
    if (localeId == mActiveLocale) return true;
    mActiveLocale = localeId;
    ++mLocaleGeneration;
    notifyLocaleChanged();
    return true;
}

void System::setKeybindingResolver(std::function<KeybindingPresentation(const std::string&)> resolver) {
    mKeybindingResolver = std::move(resolver);
    refreshKeybindings();
}

KeybindingPresentation System::resolveKeybinding(const std::string& id) const {
    return mKeybindingResolver ? mKeybindingResolver(id) : KeybindingPresentation{};
}

void System::refreshKeybindings() {
    for (Surface* surface : mSurfaces)
        if (surface) surface->keybindingsChanged();
}

void System::registerSurface(Surface& surface) const {
    mSurfaces.insert(&surface);
}

void System::unregisterSurface(Surface& surface) const {
    mSurfaces.erase(&surface);
}

void System::notifyLocaleChanged() {
    for (Surface* surface : mSurfaces)
        if (surface) surface->localeChanged();
    if (mLocaleChangedHandler) mLocaleChangedHandler(mActiveLocale);
}

LayoutBuildResult System::buildWidgetTree(const std::string& resourceId) const {
    return mSkinGeneration->buildWidgetTree(resourceId, mActiveLocale);
}

std::unique_ptr<Surface> System::createSurface(const TextMetrics& textMetrics) const {
    return std::unique_ptr<Surface>(new Surface(*this, textMetrics));
}

bool System::setLongClickDelay(std::chrono::milliseconds delay) {
    if (delay.count() <= 0) return false;
    mLongClickDelay = delay;
    return true;
}
} // namespace radia::ui
