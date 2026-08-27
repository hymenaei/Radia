/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <unordered_set>
#include <utility>
#include "diagnostic.h"
#include "layout/buildresult.h"
#include "localization.h"
#include "nativeappearance.h"
#include "style/stylesheet.h"
#include "text/keybinding.h"

namespace radia::ui {
class SkinGeneration;
class Surface;
class TextMetrics;
class OpenGLPaintContext;
class Element;
struct SvgIcon;

class PublicationCommit {
public:
    virtual ~PublicationCommit() = default;
    virtual bool commit() = 0;
};

class System {
public:
    System();
    ~System();

    bool publish(std::shared_ptr<const SkinGeneration> generation);
    bool publish(std::shared_ptr<const SkinGeneration> generation, PublicationCommit& commit);
    bool hasRelevantStyleChange(const ResourceSnapshot& current, const ResourceSnapshot& previous) const;
    LayoutBuildResult buildElementTree(const std::string& resourceId) const;
    std::unique_ptr<Surface> createSurface(const TextMetrics& textMetrics) const;
    bool setLocale(const std::string& localeId);
    void setLocaleChangedHandler(std::function<void(const std::string&)> handler) { mLocaleChangedHandler = std::move(handler); }
    void setKeybindingResolver(std::function<KeybindingPresentation(const std::string&)> resolver);
    void refreshKeybindings();
    void setNativeAppearance(std::shared_ptr<const NativeAppearance> appearance);

    std::vector<LocaleInfo> locales() const;
    const std::string& activeLocale() const { return mActiveLocale; }
    const std::string& defaultLocale() const;
    const LocaleInfo* activeLocaleInfo() const;
    LayoutDirection layoutDirection() const {
        const LocaleInfo* locale = activeLocaleInfo();
        return locale ? locale->direction : LayoutDirection::LeftToRight;
    }
    bool hasLocalizationKey(const std::string& id) const;
    std::string resolveText(const std::string& id) const;
    std::string resolveText(const LocalizedText& text) const;
    KeybindingPresentation resolveKeybinding(const std::string& id) const;
    LocalizedText t(std::string id, LocalizationArguments arguments = {}) const;
    std::string resolveMarkup(const LocalizedText& text) const;
    bool hasIcon(const std::string& name) const;
    std::uint64_t generation() const { return mGenerationNumber; }
    std::uint64_t localeGeneration() const { return mLocaleGeneration; }
    const NativeAppearance& nativeAppearance() const { return *mNativeAppearance; }

private:
    std::shared_ptr<const SkinGeneration> mSkinGeneration;
    std::shared_ptr<const NativeAppearance> mNativeAppearance;
    std::string mActiveLocale;
    std::uint64_t mGenerationNumber = 0;
    std::uint64_t mLocaleGeneration = 0;
    mutable std::unordered_set<Surface*> mSurfaces;
    std::function<void(const std::string&)> mLocaleChangedHandler;
    std::function<KeybindingPresentation(const std::string&)> mKeybindingResolver;

    const SvgIcon* icon(const std::string& name) const;
    const StyleSheet& styleSheet() const;
    void registerSurface(Surface& surface) const;
    void unregisterSurface(Surface& surface) const;
    void notifyLocaleChanged();
    bool publishImpl(std::shared_ptr<const SkinGeneration> generation, PublicationCommit* commit);
    friend class Surface;
    friend class OpenGLPaintContext;
    friend class Element;
};
} // namespace radia::ui
