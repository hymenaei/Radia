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
#include "ComputedStyle.h"
#include "Element.h"
#include "ElementInternal.h"
#include "NativeAppearance.h"
#include "OrderedChild.h"
#include "PseudoElement.h"
#include "StyleSheet.h"
#include "TreeTraversalCache.h"

namespace Core {
class Surface;
class TextMeasurer;
} // namespace Core

namespace Core::Layout {
class Pass;
} // namespace Core::Layout

namespace Core::Style {
class Pass {
public:
    using OrderedChildSnapshot = Layout::OrderedChildSnapshot;

    class TraversalScope {
    public:
        explicit TraversalScope(Pass& pass)
            : mPass(&pass) {
            mPass->beginTraversal();
        }
        TraversalScope(const TraversalScope&) = delete;
        TraversalScope& operator=(const TraversalScope&) = delete;
        ~TraversalScope() { mPass->endTraversal(); }

    private:
        Pass* mPass;
    };

    Pass(const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics, Layout::Direction direction = Layout::Direction::LeftToRight,
        NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics(), ColorSchemeContext colorSchemeContext = {});
    Pass(const Pass&) = delete;
    Pass& operator=(const Pass&) = delete;
    Pass(Pass&&) = delete;
    Pass& operator=(Pass&&) = delete;

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
    bool matches(const CSS::StyleSheet& styleSheet, const TextMeasurer& textMetrics,
        Layout::Direction direction = Layout::Direction::LeftToRight, NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics(),
        ColorSchemeContext colorSchemeContext = {}) const;
    const CSS::StyleSheet& styleSheet() const { return mStyleSheet; }
    const TextMeasurer& textMetrics() const { return mTextMeasurer; }
    Layout::Direction direction() const { return mDirection; }
    const ColorSchemeContext& colorSchemeContext() const { return mColorSchemeContext; }

private:
    friend class Surface;
    friend class Layout::Pass;

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
    const ComputedStyle& rootStyle(const Element& element) const;
    const Core::detail::LayoutContextKey& contextKey() const { return mContext; }
    Layout::TreeTraversalCache::ChildSnapshot sourceChildren(Element& parent);
    OrderedChildSnapshot orderedChildren(Element& parent);
    OrderedChildSnapshot orderedChildren(PseudoElement& parent);

    CSS::StyleSheet mStyleSheet;
    const TextMeasurer& mTextMeasurer;
    Layout::Direction mDirection = Layout::Direction::LeftToRight;
    NativeLayoutMetrics mNativeMetrics;
    ColorSchemeContext mColorSchemeContext;
    Core::detail::LayoutContextKey mContext;
    bool mInvalidated = false;
    bool mResetStorageAtBoundary = false;
    std::size_t mTraversalDepth = 0;
    std::deque<ComputedStyle> mStyleStorage;
    std::unordered_map<const Element*, CachedStyle> mStyles;
    std::unordered_map<const Element*, OrderedChildrenEntry> mOrderedChildren;
    std::unordered_map<const PseudoElement*, OrderedPseudoChildrenEntry> mOrderedPseudoChildren;
    std::uint64_t mOrderingGeneration = 1;
    Layout::TreeTraversalCache mTree;
};
} // namespace Core::Style
