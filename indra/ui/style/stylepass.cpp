/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/stylepass.h"
#include <algorithm>
#include <utility>
#include "ComputedStyleProperties.h"
#include "css/stylesheet.h"
#include "dom/element.h"
#include "paint/nativeappearance.h"
#include "text/metrics.h"

namespace radia::ui {
namespace {
void resolveMatchParentTextAlign(ComputedStyle& style, const ComputedStyle* parent) {
    if (style.textAlign() == TextAlign::MatchParent) style.setTextAlign(parent ? parent->textAlign() : TextAlign::Start);
}

detail::LayoutContextKey makeContextKey(const StyleRuleSet* ruleSet, const TextMetrics& textMetrics, std::uint64_t styleGeneration,
                                        LayoutDirection direction, const NativeLayoutMetrics& nativeMetrics,
                                        const ColorSchemeContext& colorSchemeContext) {
    detail::LayoutContextKey result{ruleSet, &textMetrics, styleGeneration, textMetrics.generation(), direction};
    result.nativeMetrics = nativeMetrics;
    result.colorSchemeContext = colorSchemeContext;
    return result;
}
} // namespace

StylePass::StylePass(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction, NativeLayoutMetrics nativeMetrics,
                     ColorSchemeContext colorSchemeContext)
    : mStyleSheet(styleSheet), mTextMetrics(textMetrics), mDirection(direction), mNativeMetrics(nativeMetrics),
      mColorSchemeContext(colorSchemeContext),
      mContext(makeContextKey(styleSheet.ruleSetIdentity(), textMetrics, styleSheet.generation(), direction, mNativeMetrics, mColorSchemeContext)) {}

bool StylePass::matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction, NativeLayoutMetrics nativeMetrics,
                        ColorSchemeContext colorSchemeContext) const {
    return mContext
        == makeContextKey(styleSheet.ruleSetIdentity(), textMetrics, styleSheet.generation(), direction, nativeMetrics, colorSchemeContext);
}

void StylePass::beginTraversal() {
    mTree.beginTraversal();
    if (mTraversalDepth++ != 0) return;
    if (mResetStorageAtBoundary) {
        mStyles.clear();
        mStyleStorage.clear();
        mOrderedChildren.clear();
        mOrderedPseudoChildren.clear();
        mInvalidated = false;
        mResetStorageAtBoundary = false;
    } else {
        compactStyles();
        compactOrderingCaches();
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

void StylePass::compactOrderingCaches() {
    for (auto it = mOrderedChildren.begin(); it != mOrderedChildren.end();)
        if (it->second.lifetime.expired()) it = mOrderedChildren.erase(it);
        else ++it;
    for (auto it = mOrderedPseudoChildren.begin(); it != mOrderedPseudoChildren.end();)
        if (it->second.lifetime.expired()) it = mOrderedPseudoChildren.erase(it);
        else ++it;
}

const ComputedStyle& StylePass::rootStyle(const Element& element) const {
    const Element* root = &element;
    while (root->parentElement()) root = root->parentElement();
    const auto found = mStyles.find(root);
    return found == mStyles.end() ? ComputedStyle::initialStyle() : mStyleStorage[found->second.storageIndex];
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

    const ConstElementVisit topologySnapshot(element);
    const std::weak_ptr<char> elementLifetime = detail::NodeAccess::lifetime(element);
    const ElementRef<const Element> styledRef(&element);
    const Element* parent = topologySnapshot.parent;
    std::optional<ComputedStyle> parentStyle;
    if (parent) {
        const ComputedStyle& inherited = style(*parent);
        if (styledRef.get() && topologySnapshot.mountValid() && topologySnapshot.topologyValid()) parentStyle = inherited;
    }
    const ConstElementVisit elementSnapshot(element);
    const std::uint64_t contextRevision = element.styleContextRevision();
    ComputedStyle resolved =
        mStyleSheet.resolveElement(element, parentStyle ? &parentStyle->customProperties : nullptr, mColorSchemeContext,
                                   parentStyle ? &parentStyle->colorScheme() : nullptr, parentStyle ? &*parentStyle : nullptr, rootStyle(element));
    const Element* current = styledRef.get();
    const auto transient = [&]() -> const ComputedStyle& {
        mStyleStorage.emplace_back(std::move(resolved));
        return mStyleStorage.back();
    };
    if (!current || !elementSnapshot.styleValid() || !topologySnapshot.mountValid() || !topologySnapshot.topologyValid()) return transient();
    element.constrainResolvedStyle(resolved);
    if (parentStyle) inheritStyle(resolved, *parentStyle);
    else resolved.textDecorationPropagation = resolved.textDecoration();
    resolveMatchParentTextAlign(resolved, parentStyle ? &*parentStyle : nullptr);
    normalizeOverflow(resolved);
    resolveStyleColors(resolved, parentStyle ? parentStyle->color().resolvedColor() : Color(0.f, 0.f, 0.f, 1.f));

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
    const ComputedStyle& parentStyle = pseudoElement.parentPseudoElement() ? style(*pseudoElement.parentPseudoElement()) : ownerStyle;
    ComputedStyle resolved = mStyleSheet.resolvePseudoElement(owner, pseudoElement.name(), &parentStyle.customProperties, mColorSchemeContext,
                                                              &parentStyle.colorScheme(), &parentStyle, rootStyle(owner));
    inheritStyle(resolved, parentStyle);
    resolveMatchParentTextAlign(resolved, &parentStyle);
    resolved.setAppearance(ownerStyle.appearance());
    normalizeOverflow(resolved);
    resolveStyleColors(resolved, parentStyle.color().resolvedColor());
    pseudoElement.setResolvedStyle(resolved);
    return resolved;
}

void StylePass::styleGeneratedPseudoElements(const Element& element, const ComputedStyle& ownerStyle) {
    if (ownerStyle.appearance() == Appearance::Auto) return;
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

StylePass::OrderedChildSnapshot StylePass::orderedChildren(Element& parent) {
    const auto lifetime = detail::NodeAccess::lifetime(parent).lock();
    const auto found = mOrderedChildren.find(&parent);
    if (found != mOrderedChildren.end()
        && found->second.lifetime.lock() == lifetime
        && found->second.childRevision == parent.mChildSnapshotRevision
        && found->second.orderingGeneration == mOrderingGeneration)
        return found->second.snapshot;

    auto result = std::make_shared<std::vector<OrderedChildRef>>();
    const ComputedStyle& parentStyle = style(parent);
    const bool includesPseudoElements = parentStyle.appearance() != Appearance::Auto;
    result->reserve(detail::nodes(parent).size() + (includesPseudoElements ? parent.generatedPseudoElements().size() : 0));
    for (detail::Node& node : detail::nodes(parent)) result->emplace_back(&node);
    if (includesPseudoElements)
        for (PseudoElement* pseudoElement : parent.generatedPseudoElements())
            if (pseudoElement) result->emplace_back(pseudoElement);
    if (isOrderModifiedContainer(parentStyle.display())) {
        std::stable_sort(result->begin(), result->end(), [this](const auto& left, const auto& right) {
            const int leftOrder = left.pseudoElement ? style(*left.pseudoElement).order().value
                : left.element()                     ? style(*left.element()).order().value
                                                     : 0;
            const int rightOrder = right.pseudoElement ? style(*right.pseudoElement).order().value
                : right.element()                      ? style(*right.element()).order().value
                                                       : 0;
            return leftOrder < rightOrder;
        });
    }
    mOrderedChildren[&parent] = {result, detail::NodeAccess::lifetime(parent), parent.mChildSnapshotRevision, mOrderingGeneration};
    return result;
}

StylePass::OrderedChildSnapshot StylePass::orderedChildren(PseudoElement& parent) {
    const auto lifetime = detail::NodeAccess::lifetime(parent.originatingElement()).lock();
    const auto found = mOrderedPseudoChildren.find(&parent);
    if (found != mOrderedPseudoChildren.end() && found->second.lifetime.lock() == lifetime && found->second.orderingGeneration == mOrderingGeneration)
        return found->second.snapshot;

    auto result = std::make_shared<std::vector<OrderedChildRef>>();
    result->reserve(parent.generatedPseudoElements().size());
    for (PseudoElement* pseudoElement : parent.generatedPseudoElements())
        if (pseudoElement) result->emplace_back(pseudoElement);
    if (isOrderModifiedContainer(style(parent).display())) {
        std::stable_sort(result->begin(), result->end(), [this](const auto& left, const auto& right) {
            return style(*left.pseudoElement).order().value < style(*right.pseudoElement).order().value;
        });
    }
    mOrderedPseudoChildren[&parent] = {result, detail::NodeAccess::lifetime(parent.originatingElement()), mOrderingGeneration};
    return result;
}
} // namespace radia::ui
