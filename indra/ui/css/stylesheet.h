/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <string_view>
#include <vector>
#include "diagnostic.h"
#include "resourceprovider.h"
#include "style/computedstyle.h"

namespace radia::ui {
class Element;
class StylePass;
struct StyleRuleSet;

enum class StyleOrigin : std::uint8_t { Default = 0, Skin = 1 };

struct StyleLayer {
    StyleOrigin origin;
    ResourceLayer resource;
};

inline constexpr std::string_view kDefaultStylesheetResourceId = "style/defaults.css";

std::string_view defaultStylesheetSource() noexcept;

struct StyleSheetLoadResult : DiagnosticResult {
    bool ok() const { return !hasErrors(); }
};

class StyleSheet {
public:
    using DependencyMap = ResourceDependencyMap;

    StyleSheet();
    ~StyleSheet();
    StyleSheet(const StyleSheet& other);
    StyleSheet& operator=(const StyleSheet& other);
    StyleSheet(StyleSheet&& other) noexcept;
    StyleSheet& operator=(StyleSheet&& other) noexcept;

    StyleSheetLoadResult loadRadia(const std::string& stylesheetSource, const std::string& sourceName = {});
    StyleSheetLoadResult loadRadiaLayers(const std::vector<StyleLayer>& layers);
    std::uint64_t generation() const;
    const DependencyMap& dependencies() const;
    bool stateAffectsLayout(ElementState state) const;
    bool stateAffectsLayout(const Element& element, ElementState state) const;
    bool stateAffectsHitTesting(ElementState state) const;
    bool stateAffectsHitTesting(const Element& element, ElementState state) const;
    bool stateAffectsDescendants(const Element& element, ElementState state) const;

    ComputedStyle resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes, uint16_t states,
                          LayoutDirection direction = LayoutDirection::LeftToRight) const;
    ComputedStyle resolveElement(const Element& element, LayoutDirection direction = LayoutDirection::LeftToRight) const;
    ComputedStyle resolvePseudoElement(const Element& owner, std::string_view pseudoElementName,
                                       LayoutDirection direction = LayoutDirection::LeftToRight) const;
    ComputedStyle resolveInline(const Element& owner, const std::string& element, const std::vector<std::string>& inlineAncestors = {},
                                LayoutDirection direction = LayoutDirection::LeftToRight) const;

private:
    friend class StylePass;

    struct Impl;
    static std::shared_ptr<Impl> makeEmptyImpl();
    const StyleRuleSet* ruleSetIdentity() const;
    std::shared_ptr<Impl> mImpl;
};
} // namespace radia::ui
