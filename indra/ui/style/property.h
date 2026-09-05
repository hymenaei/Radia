/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <optional>
#include <span>
#include <string_view>
#include <vector>
#include "css/rules.h"
#include "style/computedstyle.h"

namespace radia::ui::detail {
struct StyleCompileContext;
using StyleCompileResult = std::optional<std::vector<StyleDeclaration>>;
using StyleCompileFunction = StyleCompileResult (*)(StyleCompileContext&);
using StyleApplyFunction = void (*)(ComputedStyle&, const StyleValue&);
using StyleResetFunction = void (*)(ComputedStyle&);
using StyleSpecifyFunction = void (*)(ComputedStyle&);
using StyleInheritFunction = void (*)(ComputedStyle&, const ComputedStyle&);

enum class StylePropertyImpact : std::uint8_t { Layout = 1 << 0, Paint = 1 << 1, Inherited = 1 << 2, HitTest = 1 << 3 };

inline constexpr StylePropertyImpact operator|(StylePropertyImpact left, StylePropertyImpact right) {
    return static_cast<StylePropertyImpact>(static_cast<std::uint8_t>(left) | static_cast<std::uint8_t>(right));
}

inline constexpr bool hasImpact(StylePropertyImpact value, StylePropertyImpact flag) {
    return (static_cast<std::uint8_t>(value) & static_cast<std::uint8_t>(flag)) != 0;
}

struct StylePropertyDefinition {
    std::string_view name;
    StyleCompileFunction compile = nullptr;
    StyleApplyFunction apply = nullptr;
    StyleResetFunction reset = nullptr;
    StyleSpecifyFunction specify = nullptr;
    StyleInheritFunction inherit = nullptr;
    StylePropertyImpact impact = StylePropertyImpact::Layout;
    bool defaultOnly = false;

    bool isPaintOnly() const { return hasImpact(impact, StylePropertyImpact::Paint) && !hasImpact(impact, StylePropertyImpact::Layout); }
    bool isInherited() const { return hasImpact(impact, StylePropertyImpact::Inherited); }
    bool affectsHitTesting() const { return hasImpact(impact, StylePropertyImpact::HitTest); }
    InheritedStyleProperties inheritedBit() const { return static_cast<InheritedStyleProperties>(inheritedProperty); }

    InheritedStyleProperty inheritedProperty = InheritedStyleProperty::NotInherited;
    std::span<const std::string_view> longhands;
};

const StylePropertyDefinition* findStyleProperty(std::string_view name);
const StylePropertyDefinition* stylePropertyBegin();
const StylePropertyDefinition* stylePropertyEnd();
void applyStyleDeclaration(ComputedStyle& style, const StyleDeclaration& declaration);
} // namespace radia::ui::detail
