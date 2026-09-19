/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <deque>
#include <memory>
#include <unordered_map>
#include <vector>
#include "css/stylesheet.h"
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "layout/treecache.h"
#include "style/computedstyle.h"
#include "style/pseudoelement.h"

namespace radia::ui {
class TextMetrics;
class Surface;
class LayoutPass;

class StylePass {
public:
    using OrderedChildSnapshot = ::radia::ui::OrderedChildSnapshot;

    class TraversalScope {
    public:
        explicit TraversalScope(StylePass& pass) : mPass(&pass) { mPass->beginTraversal(); }
        TraversalScope(const TraversalScope&) = delete;
        TraversalScope& operator=(const TraversalScope&) = delete;
        ~TraversalScope() { mPass->endTraversal(); }

    private:
        StylePass* mPass;
    };

    StylePass(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction = LayoutDirection::LeftToRight,
              NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics(), ColorSchemeContext colorSchemeContext = {});
    StylePass(const StylePass&) = delete;
    StylePass& operator=(const StylePass&) = delete;
    StylePass(StylePass&&) = delete;
    StylePass& operator=(StylePass&&) = delete;

    void invalidate() {
        mInvalidated = true;
        mResetStorageAtBoundary = true;
        invalidateOrdering();
    }
    void invalidateOrdering() {
        mTree.invalidateOrdering();
        mOrderedPseudoChildren.clear();
        ++mOrderingGeneration;
    }
    void beginTraversal();
    void endTraversal();
    TraversalScope enterTraversal() { return TraversalScope(*this); }
    bool active() const { return mTraversalDepth != 0; }
    const ComputedStyle& style(const Element& element);
    ComputedStyle style(PseudoElement& pseudoElement);
    void styleGeneratedPseudoElements(const Element& element, const ComputedStyle& ownerStyle);
    bool matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction = LayoutDirection::LeftToRight,
                 NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics(), ColorSchemeContext colorSchemeContext = {}) const;
    const StyleSheet& styleSheet() const { return mStyleSheet; }
    const TextMetrics& textMetrics() const { return mTextMetrics; }
    LayoutDirection direction() const { return mDirection; }
    const ColorSchemeContext& colorSchemeContext() const { return mColorSchemeContext; }

private:
    friend class Surface;
    friend class LayoutPass;

    struct OrderedChildrenEntry {
        OrderedChildSnapshot snapshot;
        std::weak_ptr<char> lifetime;
        std::uint64_t childRevision = 0;
        std::uint64_t orderingGeneration = 0;
    };

    struct CachedStyle {
        std::size_t storageIndex = 0;
        std::weak_ptr<char> lifetime;
        std::uint64_t contextRevision = 0;
    };

    struct OrderedPseudoChildrenEntry {
        OrderedChildSnapshot snapshot;
        std::weak_ptr<char> lifetime;
        std::uint64_t orderingGeneration = 0;
    };

    void compactStyles();
    void compactOrderingCaches();
    const detail::LayoutContextKey& contextKey() const { return mContext; }
    TreeTraversalCache::ChildSnapshot sourceChildren(Element& parent);
    OrderedChildSnapshot orderedChildren(Element& parent);
    OrderedChildSnapshot orderedChildren(PseudoElement& parent);

    StyleSheet mStyleSheet;
    const TextMetrics& mTextMetrics;
    LayoutDirection mDirection = LayoutDirection::LeftToRight;
    NativeLayoutMetrics mNativeMetrics;
    ColorSchemeContext mColorSchemeContext;
    detail::LayoutContextKey mContext;
    bool mInvalidated = false;
    bool mResetStorageAtBoundary = false;
    std::size_t mTraversalDepth = 0;
    std::deque<ComputedStyle> mStyleStorage;
    std::unordered_map<const Element*, CachedStyle> mStyles;
    std::unordered_map<const Element*, OrderedChildrenEntry> mOrderedChildren;
    std::unordered_map<const PseudoElement*, OrderedPseudoChildrenEntry> mOrderedPseudoChildren;
    std::uint64_t mOrderingGeneration = 1;
    TreeTraversalCache mTree;
};
} // namespace radia::ui
