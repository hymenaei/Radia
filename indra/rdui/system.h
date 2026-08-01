/**
 * @file system.h
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

#ifndef LL_RDUI_SYSTEM_H
#define LL_RDUI_SYSTEM_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <unordered_set>
#include <utility>
#include "diagnostic.h"
#include "layout/viewresult.h"
#include "localization/localization.h"
#include "style/stylesheet.h"
#include "text/inlinecontent.h"
#include "text/source.h"

namespace rdui {
class SkinGeneration;
class Surface;
class TextMetrics;
class OpenGLPaintContext;
struct SvgIcon;

class System {
public:
    System();
    ~System();

    void publish(std::shared_ptr<const SkinGeneration> generation, const std::function<void()>& commit_documents = {});
    ViewBuildResult createView(const std::string& resource_id) const;
    std::unique_ptr<Surface> createSurface(const TextMetrics& text_metrics) const;
    bool setLongClickDelay(std::chrono::milliseconds delay);
    bool setLocale(const std::string& id);
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
    TextSource localized(std::string id) const;
    TextSource localized(LocalizationRequest request) const;
    bool hasIcon(const std::string& name) const;
    bool sameReloadInputs(const ResourceSnapshot& left, const ResourceSnapshot& right) const;
    std::uint64_t generation() const { return mGenerationNumber; }
    std::uint64_t localeGeneration() const { return mLocaleGeneration; }
    std::chrono::milliseconds longClickDelay() const { return mLongClickDelay; }

private:
    std::shared_ptr<const SkinGeneration> mSkinGeneration;
    std::string mActiveLocale;
    std::uint64_t mGenerationNumber = 0;
    std::uint64_t mLocaleGeneration = 0;
    std::chrono::milliseconds mLongClickDelay{500};
    mutable std::unordered_set<Surface*> mSurfaces;
    std::function<void(const std::string&)> mLocaleChangedHandler;
    std::function<KeybindingPresentation(const std::string&)> mKeybindingResolver;

    const SvgIcon* icon(const std::string& name) const;
    const StyleSheet& styleSheet() const;
    void registerSurface(Surface& surface) const;
    void unregisterSurface(Surface& surface) const;
    void notifyLocaleChanged();
    friend class Surface;
    friend class OpenGLPaintContext;
};
} // namespace rdui
#endif // LL_RDUI_SYSTEM_H
