/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "StyleSheet.h"
#include <algorithm>
#include <cmath>
#include <cstddef>
#include <tuple>
#include <utility>
#include "CSSRules.h"

namespace Core::CSS {

std::shared_ptr<StyleSheet::Impl> StyleSheet::makeEmptyImpl() { return std::make_shared<Impl>(); }

StyleSheet::StyleSheet()
    : mImpl(makeEmptyImpl()) {}
StyleSheet::~StyleSheet() = default;
StyleSheet::StyleSheet(const StyleSheet& other)
    : mImpl(other.mImpl ? other.mImpl : makeEmptyImpl())
    , mFontFaces(other.mFontFaces) {}
StyleSheet& StyleSheet::operator=(const StyleSheet& other) {
    if (this != &other) {
        auto replacement = other.mImpl ? std::make_shared<Impl>(*other.mImpl) : makeEmptyImpl();
        auto replacementFontFaces = other.mFontFaces;
        const std::uint64_t currentGeneration = mImpl ? mImpl->generation : std::uint64_t {0};
        replacement->generation = std::max(currentGeneration, replacement->generation) + std::uint64_t {1};
        mImpl = std::move(replacement);
        mFontFaces = std::move(replacementFontFaces);
    }
    return *this;
}
StyleSheet::StyleSheet(StyleSheet&& other) noexcept
    : mImpl(std::move(other.mImpl)) {
    if (!mImpl)
        mImpl = makeEmptyImpl();
    mFontFaces = std::move(other.mFontFaces);
    other.mImpl = makeEmptyImpl();
}
StyleSheet& StyleSheet::operator=(StyleSheet&& other) noexcept {
    if (this != &other) {
        auto replacement = std::move(other.mImpl);
        if (!replacement)
            replacement = makeEmptyImpl();
        const std::uint64_t currentGeneration = mImpl ? mImpl->generation : std::uint64_t {0};
        replacement->generation = std::max(currentGeneration, replacement->generation) + std::uint64_t {1};
        mImpl = std::move(replacement);
        mFontFaces = std::move(other.mFontFaces);
        other.mImpl = makeEmptyImpl();
    }
    return *this;
}

std::uint64_t StyleSheet::generation() const { return mImpl->generation; }

const std::vector<FontFace>& StyleSheet::fontFaces() const { return mFontFaces; }

std::vector<const FontFace*> fontFacesInMatchOrder(const std::vector<FontFace>& faces, const std::string& family,
    const Style::FontSelectionRequest& request) {
    using Rank = std::tuple<int, float, int, int, float>;
    const auto widthRank = [&request](float width) {
        const bool preferred = request.width.percentage <= 100.f ? width <= request.width.percentage : width >= request.width.percentage;
        return std::pair {preferred ? 0 : 1, std::abs(width - request.width.percentage)};
    };
    const auto styleRank = [&request](Style::FontStyle style) {
        if (style == request.style)
            return 0;
        if (request.style == Style::FontStyle::Normal)
            return style == Style::FontStyle::Oblique ? 1 : 2;
        if (request.style == Style::FontStyle::Italic)
            return style == Style::FontStyle::Oblique ? 1 : 2;
        return style == Style::FontStyle::Italic ? 1 : 2;
    };
    const auto weightRank = [&request](float weight) {
        const float desired = request.weight.value;
        if (desired >= 400.f && desired <= 500.f) {
            if (weight == desired)
                return std::pair {0, 0.f};
            if (weight > desired && weight <= 500.f)
                return std::pair {1, weight - desired};
            if (weight < desired)
                return std::pair {2, desired - weight};
            return std::pair {3, weight - 500.f};
        }
        if (desired < 400.f)
            return weight <= desired ? std::pair {0, desired - weight} : std::pair {1, weight - desired};
        return weight >= desired ? std::pair {0, weight - desired} : std::pair {1, desired - weight};
    };

    const auto rank = [&](const FontFace& face) {
        const auto [widthDirection, widthDistance] = widthRank(face.selection.width.percentage);
        const auto [weightDirection, weightDistance] = weightRank(face.selection.weight.value);
        return Rank {widthDirection, widthDistance, styleRank(face.selection.style), weightDirection, weightDistance};
    };

    std::vector<const FontFace*> candidates;
    for (auto face = faces.rbegin(); face != faces.rend(); ++face) {
        if (LLStringUtil::compareInsensitive(face->family, family) != 0)
            continue;
        candidates.push_back(&*face);
    }
    std::stable_sort(candidates.begin(), candidates.end(), [&](const FontFace* left, const FontFace* right) {
        return rank(*left) < rank(*right);
    });
    return candidates;
}

const StyleRuleSet* StyleSheet::ruleSetIdentity() const { return &mImpl->ruleSet; }

const StyleSheet::DependencyMap& StyleSheet::dependencies() const { return mImpl->ruleSet.dependencies(); }

const std::vector<StyleResourceReference>& StyleSheet::resourceReferences() const { return mImpl->ruleSet.resourceReferences(); }

bool StyleSheet::pseudoClassAffectsLayout(PseudoClass pseudoClass) const { return mImpl->ruleSet.pseudoClassAffectsLayout(pseudoClass); }

bool StyleSheet::pseudoClassAffectsLayout(const Element& element, PseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsLayout(element, pseudoClass);
}

bool StyleSheet::pseudoClassAffectsHitTesting(PseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsHitTesting(pseudoClass);
}

bool StyleSheet::pseudoClassAffectsHitTesting(const Element& element, PseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsHitTesting(element, pseudoClass);
}

bool StyleSheet::pseudoClassAffectsDescendants(const Element& element, PseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsDescendants(element, pseudoClass);
}

bool StyleSheet::pseudoClassAffectsFollowingSiblings(const Element& element, PseudoClass pseudoClass) const {
    return mImpl->ruleSet.pseudoClassAffectsFollowingSiblings(element, pseudoClass);
}

StyleRuleSet StyleModel::build() && { return StyleRuleSet(std::move(*this)); }
} // namespace Core::CSS
