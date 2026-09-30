/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include <vector>
#include "Diagnostic.h"
#include "LayoutGeometry.h"
#include "LocalizedText.h"
#include "ResourceProvider.h"

namespace Core {
struct LocaleInfo {
    std::string localeId;
    std::string name;
    Layout::Direction direction = Layout::Direction::LeftToRight;
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
} // namespace Core
