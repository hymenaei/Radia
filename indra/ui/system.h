/**
 * @file system.h
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

#ifndef RD_SYSTEM_H
#define RD_SYSTEM_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <unordered_set>
#include <utility>
#include "diagnostic.h"
#include "layout/buildresult.h"
#include "localization/localization.h"
#include "style/stylesheet.h"
#include "text/inlinecontent.h"
#include "text/source.h"

namespace radia::viewer::ui {
class SkinReloadCoordinator;
} // namespace radia::viewer::ui

namespace radia::ui {
class SkinGeneration;
class Surface;
class TextMetrics;
class OpenGLPaintContext;
struct SvgIcon;

class PublicationCommit {
public:
    virtual ~PublicationCommit() = default;
    virtual bool commit() = 0;
};

class System {
public:
    static constexpr std::chrono::milliseconds defaultLongClickDelay() { return std::chrono::milliseconds{500}; }

    System();
    ~System();

    bool publish(std::shared_ptr<const SkinGeneration> generation);
    bool publish(std::shared_ptr<const SkinGeneration> generation, PublicationCommit& commit);
    LayoutBuildResult buildWidgetTree(const std::string& resourceId) const;
    std::unique_ptr<Surface> createSurface(const TextMetrics& textMetrics) const;
    bool setLongClickDelay(std::chrono::milliseconds delay);
    bool setLocale(const std::string& localeId);
    void setLocaleChangedHandler(std::function<void(const std::string&)> handler) { mLocaleChangedHandler = std::move(handler); }
    void setKeybindingResolver(std::function<KeybindingPresentation(const std::string&)> resolver);
    void refreshKeybindings();

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
    std::string resolveText(const LocalizationRequest& request) const;
    InlineContent resolveContent(const LocalizationRequest& request) const;
    KeybindingPresentation resolveKeybinding(const std::string& id) const;
    TextSource localize(std::string id) const;
    TextSource localize(LocalizationRequest request) const;
    bool hasIcon(const std::string& name) const;
    std::uint64_t generation() const { return mGenerationNumber; }
    std::uint64_t localeGeneration() const { return mLocaleGeneration; }
    std::chrono::milliseconds longClickDelay() const { return mLongClickDelay; }

private:
    std::shared_ptr<const SkinGeneration> mSkinGeneration;
    std::string mActiveLocale;
    std::uint64_t mGenerationNumber = 0;
    std::uint64_t mLocaleGeneration = 0;
    std::chrono::milliseconds mLongClickDelay{defaultLongClickDelay()};
    mutable std::unordered_set<Surface*> mSurfaces;
    std::function<void(const std::string&)> mLocaleChangedHandler;
    std::function<KeybindingPresentation(const std::string&)> mKeybindingResolver;

    const SvgIcon* icon(const std::string& name) const;
    const StyleSheet& styleSheet() const;
    void registerSurface(Surface& surface) const;
    void unregisterSurface(Surface& surface) const;
    void notifyLocaleChanged();
    bool publishImpl(std::shared_ptr<const SkinGeneration> generation, PublicationCommit* commit);
    friend class ::radia::viewer::ui::SkinReloadCoordinator;
    friend class Surface;
    friend class OpenGLPaintContext;
};
} // namespace radia::ui
#endif // RD_SYSTEM_H
