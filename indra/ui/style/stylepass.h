/**
 * @file stylepass.h
 * @brief Implements cached retained-tree style resolution.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA 02110-1301 USA
 * $/LicenseInfo$
 */

#ifndef RD_STYLE_STYLEPASS_H
#define RD_STYLE_STYLEPASS_H

#include <cstddef>
#include <cstdint>
#include <deque>
#include <memory>
#include <unordered_map>
#include <vector>
#include "layout/treecache.h"
#include "style/style.h"
#include "style/stylesheet.h"
#include "widgets/widget.h"

namespace radia::ui {
class TextMetrics;

class StylePass {
public:
    using ChildSnapshot = TreeTraversalCache::ChildSnapshot;
    class TraversalScope {
    public:
        explicit TraversalScope(StylePass& pass) : mPass(&pass) { mPass->beginTraversal(); }
        TraversalScope(const TraversalScope&) = delete;
        TraversalScope& operator=(const TraversalScope&) = delete;
        ~TraversalScope() { mPass->endTraversal(); }

    private:
        StylePass* mPass;
    };

    StylePass(const StyleSheet& styleSheet, const TextMetrics& textMetrics);
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
    const Style& style(const Widget& widget);
    ChildSnapshot orderedChildren(const Widget& parent);
    ChildSnapshot sourceChildren(const Widget& parent);
    const LayoutContextKey& contextKey() const { return mContext; }
    bool matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics) const;
    const StyleSheet& styleSheet() const { return mStyleSheet; }
    const TextMetrics& textMetrics() const { return mTextMetrics; }

private:
    struct CachedStyle {
        std::size_t storageIndex = 0;
        std::weak_ptr<char> lifetime;
        std::uint64_t contextRevision = 0;
    };

    void compactStyles();

    StyleSheet mStyleSheet;
    const TextMetrics& mTextMetrics;
    LayoutContextKey mContext;
    bool mInvalidated = false;
    bool mResetStorageAtBoundary = false;
    std::size_t mTraversalDepth = 0;
    std::deque<Style> mStyleStorage;
    std::unordered_map<const Widget*, CachedStyle> mStyles;
    TreeTraversalCache mTree;
};
} // namespace radia::ui
#endif // RD_STYLE_STYLEPASS_H
