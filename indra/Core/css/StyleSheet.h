/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <initializer_list>
#include <map>
#include <memory>
#include <set>
#include <string>
#include <string_view>
#include <variant>
#include <vector>
#include "CSSPseudoSelectors.h"
#include "ComputedStyle.h"
#include "Diagnostic.h"
#include "ResourceProvider.h"
#include "UserAgentStyleSheet.h"

namespace Core {
class Element;
class SkinCompiler;
namespace Style {
class Pass;
} // namespace Style
} // namespace Core

namespace Core::CSS {
struct StyleRuleSet;

enum class StyleOrigin : std::uint8_t {
    UserAgent = 0,
    Skin = 1
};

struct StyleLayer {
    StyleOrigin origin;
    ResourceLayer resource;
};

struct StyleResourceReference {
    std::string value;
    bool optional = false;
    bool cursor = false;
};

struct FontFaceURL {
    std::string url;
    ResourceId id;
};

struct FontFaceLocal {
    std::string name;
};

struct FontFaceSource {
    std::variant<FontFaceURL, FontFaceLocal> value;
    std::string sourceName;
    std::size_t line = 1;
    std::size_t column = 1;
};

struct FontFace {
    std::string family;
    Style::FontSelectionRequest selection;
    std::vector<FontFaceSource> sources;
    StyleOrigin origin = StyleOrigin::Skin;
    std::string sourceName;
    std::size_t line = 1;
    std::size_t column = 1;
};

std::vector<const FontFace*> fontFacesInMatchOrder(const std::vector<FontFace>& faces, const std::string& family,
    const Style::FontSelectionRequest& request);

inline constexpr std::string_view kUserAgentStyleSheetId = "style/ua.css";

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
    const std::vector<FontFace>& fontFaces() const;
    const DependencyMap& dependencies() const;
    const std::vector<StyleResourceReference>& resourceReferences() const;
    bool pseudoClassAffectsLayout(PseudoClass pseudoClass) const;
    bool pseudoClassAffectsLayout(const Element& element, PseudoClass pseudoClass) const;
    bool pseudoClassAffectsHitTesting(PseudoClass pseudoClass) const;
    bool pseudoClassAffectsHitTesting(const Element& element, PseudoClass pseudoClass) const;
    bool pseudoClassAffectsDescendants(const Element& element, PseudoClass pseudoClass) const;
    bool pseudoClassAffectsFollowingSiblings(const Element& element, PseudoClass pseudoClass) const;

    Style::ComputedStyle resolve(const std::string& element, const std::string& id, const std::set<std::string>& classes,
        std::initializer_list<PseudoClass> matchingPseudoClasses = {}) const;
    Style::ComputedStyle resolveElement(const Element& element) const;
    Style::ComputedStyle resolvePseudoElement(const Element& owner, std::string_view pseudoElementName) const;
    Style::ComputedStyle resolveInline(const Element& owner, const std::string& element,
        const std::vector<std::string>& inlineAncestors = {}) const;

private:
    friend class Core::SkinCompiler;
    friend class Style::Pass;

    Style::ComputedStyle resolveElement(const Element& element, const Style::CustomPropertyMap* inheritedCustomProperties,
        Style::ColorSchemeContext colorSchemeContext, const Style::ColorScheme* inheritedColorScheme,
        const Style::ComputedStyle* parentStyle, const Style::ComputedStyle& rootStyle) const;
    Style::ComputedStyle resolvePseudoElement(const Element& owner, std::string_view pseudoElementName,
        const Style::CustomPropertyMap* inheritedCustomProperties, Style::ColorSchemeContext colorSchemeContext,
        const Style::ColorScheme* inheritedColorScheme, const Style::ComputedStyle* parentStyle,
        const Style::ComputedStyle& rootStyle) const;

    struct Impl;
    static std::shared_ptr<Impl> makeEmptyImpl();
    const StyleRuleSet* ruleSetIdentity() const;
    std::shared_ptr<Impl> mImpl;
    std::vector<FontFace> mFontFaces;
};
} // namespace Core::CSS
