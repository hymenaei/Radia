/**
 * @file rduilocalization.h
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

#ifndef LL_RDUI_LOCALIZATION_H
#define LL_RDUI_LOCALIZATION_H

#include <memory>
#include <string>
#include <vector>
#include "rduidiagnostic.h"
#include "rduiinlinecontent.h"
#include "rduilocalizationvalue.h"
#include "rduiresourceprovider.h"
#include "rduitypes.h"

namespace rdui {
struct LocaleInfo {
    std::string id;
    std::string name;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    std::string fallback;
};

struct LocalizationLoadResult : DiagnosticResult {
    bool ok() const { return !hasErrors(); }
};

class LocalizationCatalog {
public:
    LocalizationCatalog();
    ~LocalizationCatalog();
    LocalizationCatalog(LocalizationCatalog&&) noexcept;
    LocalizationCatalog& operator=(LocalizationCatalog&&) noexcept;
    LocalizationCatalog(const LocalizationCatalog&) = delete;
    LocalizationCatalog& operator=(const LocalizationCatalog&) = delete;

    LocalizationLoadResult loadYaml(const std::string& yaml, const std::string& source_name = {});
    LocalizationLoadResult loadYamlLayers(const std::vector<ResourceLayer>& layers);

    std::vector<LocaleInfo> locales() const;
    const std::string& defaultLocaleId() const;
    const LocaleInfo* locale(const std::string& id) const;
    bool containsLocale(const std::string& id) const;
    bool containsDefaultString(const std::string& id) const;
    bool pluralCapable(const std::string& id) const;

    InlineContent resolve(const std::string& locale_id, const LocalizationRequest& request) const;
    std::string get(const std::string& locale_id, const LocalizationRequest& request) const;
    std::string get(const std::string& locale_id, const std::string& string_id) const;

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui
#endif // LL_RDUI_LOCALIZATION_H
