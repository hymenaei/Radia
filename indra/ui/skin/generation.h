/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include <vector>
#include "layout/buildresult.h"
#include "localization.h"
#include "resourceprovider.h"
#include "style/stylesheet.h"

namespace radia::ui {
class SkinCompiler;
class System;
struct SvgIcon;

class SkinGeneration final {
public:
    ~SkinGeneration();

    SkinGeneration(const SkinGeneration&) = delete;
    SkinGeneration& operator=(const SkinGeneration&) = delete;

    ResourceBuildResult buildElementTree(const ResourceId& id, const std::string& locale) const;

private:
    struct Impl;
    explicit SkinGeneration(std::unique_ptr<Impl> implementation);

    static std::shared_ptr<const SkinGeneration> empty();
    DiagnosticResult validateElementDefaults(const std::string& elementName) const;
    void validateIconReferences(Element& element, ResourceBuildResult& result) const;
    std::vector<LocaleInfo> locales() const;
    const std::string& defaultLocale() const;
    const LocaleInfo* locale(const std::string& id) const;
    bool containsLocale(const std::string& id) const;
    bool hasLocalizationKey(const std::string& id) const;
    std::string resolveHTML(const std::string& locale, const LocalizedText& text) const;
    std::string resolveText(const std::string& locale, const LocalizedText& text) const;
    const StyleSheet& styleSheet() const;
    const SvgIcon* icon(const std::string& name) const;

    std::unique_ptr<Impl> mImpl;

    friend class SkinCompiler;
    friend class System;
};
} // namespace radia::ui
