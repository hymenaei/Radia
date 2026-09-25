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
              NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics());
    StylePass(const StylePass&) = delete;
    StylePass& operator=(const StylePass&) = delete;
    StylePass(StylePass&&) = delete;
    StylePass& operator=(StylePass&&) = delete;

    void invalidate() {
        mInvalidated = true;
        mResetStorageAtBoundary = true;
        mTree.invalidateOrdering();
    }
    void invalidateOrdering() { mTree.invalidateOrdering(); }
    void beginTraversal();
    void endTraversal();
    TraversalScope enterTraversal() { return TraversalScope(*this); }
    bool active() const { return mTraversalDepth != 0; }
    const ComputedStyle& style(const Element& element);
    ComputedStyle style(PseudoElement& pseudoElement);
    void styleGeneratedPseudoElements(const Element& element, const ComputedStyle& ownerStyle);
    bool matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics, LayoutDirection direction = LayoutDirection::LeftToRight,
                 NativeLayoutMetrics nativeMetrics = defaultNativeLayoutMetrics()) const;
    const StyleSheet& styleSheet() const { return mStyleSheet; }
    const TextMetrics& textMetrics() const { return mTextMetrics; }
    LayoutDirection direction() const { return mDirection; }

private:
    friend class Surface;
    friend class LayoutPass;

    struct CachedStyle {
        std::size_t storageIndex = 0;
        std::weak_ptr<char> lifetime;
        std::uint64_t contextRevision = 0;
    };

    void compactStyles();
    const detail::LayoutContextKey& contextKey() const { return mContext; }
    TreeTraversalCache::ChildSnapshot sourceChildren(Element& parent);

    StyleSheet mStyleSheet;
    const TextMetrics& mTextMetrics;
    LayoutDirection mDirection = LayoutDirection::LeftToRight;
    NativeLayoutMetrics mNativeMetrics;
    detail::LayoutContextKey mContext;
    bool mInvalidated = false;
    bool mResetStorageAtBoundary = false;
    std::size_t mTraversalDepth = 0;
    std::deque<ComputedStyle> mStyleStorage;
    std::unordered_map<const Element*, CachedStyle> mStyles;
    TreeTraversalCache mTree;
};
} // namespace radia::ui
