/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include <vector>
#include "diagnostic.h"
#include "localizedtext.h"
#include "resourceprovider.h"
#include "types.h"

namespace radia::ui {
struct LocaleInfo {
    std::string localeId;
    std::string name;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    std::string fallback;
};

class LocalizationCatalog {
public:
    LocalizationCatalog();
    ~LocalizationCatalog();
    LocalizationCatalog(LocalizationCatalog&&) noexcept;
    LocalizationCatalog& operator=(LocalizationCatalog&&) noexcept;
    LocalizationCatalog(const LocalizationCatalog&) = delete;
    LocalizationCatalog& operator=(const LocalizationCatalog&) = delete;

    DiagnosticResult loadYaml(const std::string& yaml, const std::string& sourceName = {});
    DiagnosticResult loadYamlLayers(const std::vector<ResourceLayer>& layers);

    std::vector<LocaleInfo> locales() const;
    const std::string& defaultLocaleId() const;
    const LocaleInfo* locale(const std::string& localeId) const;
    bool containsLocale(const std::string& localeId) const;
    bool containsDefaultString(const std::string& stringKey) const;

    std::string resolveHTML(const std::string& localeId, const LocalizedText& text) const;
    std::string resolveText(const std::string& localeId, const LocalizedText& text) const;
    std::string resolveText(const std::string& localeId, const std::string& stringKey) const;

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace radia::ui
