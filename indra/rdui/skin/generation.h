/**
 * @file generation.h
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

#ifndef LL_RDUI_SKIN_GENERATION_H
#define LL_RDUI_SKIN_GENERATION_H

#include <memory>
#include <string>
#include <vector>
#include "layout/viewresult.h"
#include "localization/localization.h"
#include "style/stylesheet.h"

namespace rdui {
class SkinCompiler;
class System;
struct SvgIcon;

class SkinGeneration final {
public:
    ~SkinGeneration();

    SkinGeneration(const SkinGeneration&) = delete;
    SkinGeneration& operator=(const SkinGeneration&) = delete;

    ViewBuildResult createView(const std::string& resource_id, const std::string& locale) const;

private:
    struct Impl;
    explicit SkinGeneration(std::unique_ptr<Impl> implementation);

    static std::shared_ptr<const SkinGeneration> empty();
    DiagnosticResult validateWidgetDefaults(const std::string& element) const;
    void validateIconReferences(Widget& widget, ViewBuildResult& result) const;
    std::vector<LocaleInfo> locales() const;
    const std::string& defaultLocale() const;
    const LocaleInfo* locale(const std::string& id) const;
    bool containsLocale(const std::string& id) const;
    bool hasLocalizationKey(const std::string& id) const;
    InlineContent resolveContent(const std::string& locale, const LocalizationRequest& request) const;
    std::string resolveText(const std::string& locale, const LocalizationRequest& request) const;
    const StyleSheet& styleSheet() const;
    const SvgIcon* icon(const std::string& name) const;

    std::unique_ptr<Impl> mImpl;

    friend class SkinCompiler;
    friend class System;
};
} // namespace rdui
#endif // LL_RDUI_SKIN_GENERATION_H
