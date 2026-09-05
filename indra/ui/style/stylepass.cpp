/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/stylepass.h"
#include <algorithm>
#include <utility>
#include "css/stylesheet.h"
#include "dom/element.h"
#include "nativeappearance.h"
#include "text/metrics.h"

namespace radia::ui {
namespace {
detail::LayoutContextKey makeContextKey(const StyleRuleSet* ruleSet, const TextMetrics& textMetrics, std::uint64_t styleGeneration,
                                        LayoutDirection direction, const NativeLayoutMetrics& nativeMetrics) {
    detail::LayoutContextKey result{ruleSet, &textMetrics, styleGeneration, textMetrics.generation(), direction};
    result.nativeMetrics = nativeMetrics;
    return result;
}
} // namespace

StylePass::StylePass(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction, NativeLayoutMetrics nativeMetrics)
    : mStyleSheet(styleSheet), mTextMetrics(textMetrics), mDirection(direction), mNativeMetrics(nativeMetrics),
      mContext(makeContextKey(styleSheet.ruleSetIdentity(), textMetrics, styleSheet.generation(), direction, mNativeMetrics)) {}

bool StylePass::matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction,
                        NativeLayoutMetrics nativeMetrics) const {
    return mContext == makeContextKey(styleSheet.ruleSetIdentity(), textMetrics, styleSheet.generation(), direction, nativeMetrics);
}

void StylePass::beginTraversal() {
    mTree.beginTraversal();
    if (mTraversalDepth++ != 0) return;
    if (mResetStorageAtBoundary) {
        mStyles.clear();
        mStyleStorage.clear();
        mInvalidated = false;
        mResetStorageAtBoundary = false;
    } else {
        compactStyles();
    }
}

void StylePass::compactStyles() {
    constexpr std::size_t kStorageSlack = 32;
    if (mStyleStorage.size() <= mStyles.size() * 2 + kStorageSlack) return;

    std::deque<ComputedStyle> compacted;
    std::unordered_map<const Element*, CachedStyle> styles;
    styles.reserve(mStyles.size());
    for (const auto& [element, cached] : mStyles) {
        compacted.push_back(mStyleStorage[cached.storageIndex]);
        styles.emplace(element, CachedStyle{compacted.size() - 1, cached.lifetime, cached.contextRevision});
    }
    mStyleStorage.swap(compacted);
    mStyles.swap(styles);
}

void StylePass::endTraversal() {
    llassert(mTraversalDepth != 0);
    if (mTraversalDepth) --mTraversalDepth;
    mTree.endTraversal();
}

const ComputedStyle& StylePass::style(const Element& element) {
    if (mInvalidated) {
        mStyles.clear();
        mInvalidated = false;
    }
    const auto found = mStyles.find(&element);
    const auto lifetime = detail::NodeAccess::lifetime(element).lock();
    if (found != mStyles.end() && found->second.lifetime.lock() == lifetime && found->second.contextRevision == element.styleContextRevision())
        return mStyleStorage[found->second.storageIndex];
    if (found != mStyles.end()) mStyles.erase(found);

    const ConstElementVisit elementSnapshot(element);
    const std::weak_ptr<char> elementLifetime = detail::NodeAccess::lifetime(element);
    const ElementRef<const Element> styledRef(&element);
    const Element* parent = elementSnapshot.parent;
    const std::uint64_t contextRevision = element.styleContextRevision();
    ComputedStyle resolved = mStyleSheet.resolveElement(element, mDirection);
    const Element* current = styledRef.get();
    const auto transient = [&]() -> const ComputedStyle& {
        mStyleStorage.emplace_back(std::move(resolved));
        return mStyleStorage.back();
    };
    if (!current || !elementSnapshot.styleValid()) return transient();
    if (parent) {
        const ConstElementVisit parentSnapshot(*parent);
        const ComputedStyle& parentStyle = style(*parent);
        current = styledRef.get();
        const Element* currentParent = parentSnapshot.get();
        if (!current || !currentParent || !elementSnapshot.styleValid() || !parentSnapshot.styleValid()) return transient();
        inheritStyle(resolved, parentStyle);
    }
    element.constrainResolvedStyle(resolved);
    normalizeOverflow(resolved);
    resolveLightDarkColors(resolved);
    resolveCurrentColors(resolved);

    current = styledRef.get();
    if (!current || !elementSnapshot.styleValid()) return transient();
    const std::uint64_t finalContextRevision = current->styleContextRevision();
    mStyleStorage.emplace_back(std::move(resolved));
    const std::size_t storageIndex = mStyleStorage.size() - 1;
    mStyles[&element] = CachedStyle{storageIndex, elementLifetime, finalContextRevision == contextRevision ? contextRevision : finalContextRevision};
    return mStyleStorage[storageIndex];
}

ComputedStyle StylePass::style(PseudoElement& pseudoElement) {
    const Element& owner = pseudoElement.originatingElement();
    const ComputedStyle& ownerStyle = style(owner);
    ComputedStyle resolved = mStyleSheet.resolvePseudoElement(owner, pseudoElement.name(), mDirection);
    const ComputedStyle& parentStyle = pseudoElement.parentPseudoElement() ? style(*pseudoElement.parentPseudoElement()) : ownerStyle;
    inheritStyle(resolved, parentStyle);
    resolved.appearance = ownerStyle.appearance;
    normalizeOverflow(resolved);
    resolveLightDarkColors(resolved);
    resolveCurrentColors(resolved);
    pseudoElement.setResolvedStyle(resolved);
    return resolved;
}

void StylePass::styleGeneratedPseudoElements(const Element& element, const ComputedStyle& ownerStyle) {
    if (ownerStyle.appearance != AppearanceMode::Base) return;
    const auto stylePseudoElementTree = [this](auto&& self, PseudoElement& pseudoElement) -> void {
        style(pseudoElement);
        for (PseudoElement* child : pseudoElement.generatedPseudoElements())
            if (child) self(self, *child);
    };
    for (PseudoElement* pseudoElement : element.generatedPseudoElements())
        if (pseudoElement) stylePseudoElementTree(stylePseudoElementTree, *pseudoElement);
}

TreeTraversalCache::ChildSnapshot StylePass::sourceChildren(Element& parent) {
    return mTree.sourceChildren(parent);
}
} // namespace radia::ui
