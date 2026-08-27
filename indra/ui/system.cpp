/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "system.h"
#include <set>
#include "skin/generation.h"
#include "surface/surface.h"

namespace {
bool equalStyleInputs(const radia::ui::ResourceSnapshot& left, const radia::ui::ResourceSnapshot& right,
                      const radia::ui::ResourceDependencyMap& dependencies) {
    if (left.resources() != right.resources()) return false;
    const auto& leftResources = left.layeredResources();
    const auto& rightResources = right.layeredResources();
    if (leftResources.size() != rightResources.size()) return false;

    std::set<std::string> relevantSources;
    for (const auto& [source, imports] : dependencies) {
        relevantSources.insert(source);
        relevantSources.insert(imports.begin(), imports.end());
    }

    for (const auto& [resourceId, leftLayers] : leftResources) {
        const auto rightResource = rightResources.find(resourceId);
        if (rightResource == rightResources.end()) return false;
        const auto& rightLayers = rightResource->second;
        if (resourceId != "skin.css") {
            if (leftLayers != rightLayers) return false;
            continue;
        }
        if (leftLayers.size() != rightLayers.size()) return false;
        for (std::size_t index = 0; index < leftLayers.size(); ++index) {
            const radia::ui::ResourceLayer& leftLayer = leftLayers[index];
            const radia::ui::ResourceLayer& rightLayer = rightLayers[index];
            if (leftLayer.sourceName != rightLayer.sourceName
                || leftLayer.source != rightLayer.source
                || leftLayer.entrypoint != rightLayer.entrypoint)
                return false;

            for (const auto& [moduleId, source] : leftLayer.modules) {
                if (!relevantSources.contains(leftLayer.sourceNameFor(moduleId))) continue;
                const auto rightModule = rightLayer.modules.find(moduleId);
                if (rightModule == rightLayer.modules.end() || rightModule->second != source) return false;
            }
            for (const auto& [moduleId, source] : rightLayer.modules) {
                if (!relevantSources.contains(rightLayer.sourceNameFor(moduleId))) continue;
                const auto leftModule = leftLayer.modules.find(moduleId);
                if (leftModule == leftLayer.modules.end() || leftModule->second != source) return false;
            }
        }
    }
    return true;
}
} // namespace

namespace radia::ui {
System::System() : mSkinGeneration(SkinGeneration::empty()), mNativeAppearance(std::make_shared<FallbackNativeAppearance>()) {}

System::~System() = default;

bool System::publish(std::shared_ptr<const SkinGeneration> generation) {
    return publishImpl(std::move(generation), nullptr);
}

bool System::publish(std::shared_ptr<const SkinGeneration> generation, PublicationCommit& commit) {
    return publishImpl(std::move(generation), &commit);
}

bool System::hasRelevantStyleChange(const ResourceSnapshot& current, const ResourceSnapshot& previous) const {
    return !equalStyleInputs(current, previous, styleSheet().dependencies());
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
    return resolveText(LocalizedText(id));
}

std::string System::resolveText(const LocalizedText& text) const {
    return mSkinGeneration->resolveText(mActiveLocale, text);
}

std::string System::resolveMarkup(const LocalizedText& text) const {
    return mSkinGeneration->resolveMarkup(mActiveLocale, text);
}

LocalizedText System::t(std::string id, LocalizationArguments arguments) const {
    return LocalizedText(std::move(id), std::move(arguments));
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

void System::setNativeAppearance(std::shared_ptr<const NativeAppearance> appearance) {
    if (!appearance) appearance = std::make_shared<FallbackNativeAppearance>();
    if (mNativeAppearance == appearance && mNativeAppearance->revision() == appearance->revision()) return;
    mNativeAppearance = std::move(appearance);
    for (Surface* surface : mSurfaces)
        if (surface) surface->nativeAppearanceChanged();
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

LayoutBuildResult System::buildElementTree(const std::string& resourceId) const {
    return mSkinGeneration->buildElementTree(resourceId, mActiveLocale);
}

std::unique_ptr<Surface> System::createSurface(const TextMetrics& textMetrics) const {
    return std::unique_ptr<Surface>(new Surface(*this, textMetrics));
}
} // namespace radia::ui
