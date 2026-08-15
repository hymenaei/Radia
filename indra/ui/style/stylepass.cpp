/**
 * @file stylepass.cpp
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

#include "linden_common.h"
#include "style/stylepass.h"
#include <algorithm>
#include <utility>
#include "style/stylesheet.h"
#include "text/metrics.h"
#include "widgets/widget.h"

namespace radia::ui {
namespace {
LayoutContextKey makeContextKey(const StyleSheet& styleSheet, const TextMetrics& textMetrics) {
    return {&styleSheet, &textMetrics, styleSheet.generation(), textMetrics.generation()};
}
} // namespace

StylePass::StylePass(const StyleSheet& styleSheet, const TextMetrics& textMetrics)
    : mStyleSheet(styleSheet), mTextMetrics(textMetrics), mContext(makeContextKey(styleSheet, textMetrics)) {}

bool StylePass::matches(const StyleSheet& styleSheet, const TextMetrics& textMetrics) const {
    return mContext == makeContextKey(styleSheet, textMetrics);
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

    std::deque<Style> compacted;
    std::unordered_map<const Widget*, CachedStyle> styles;
    styles.reserve(mStyles.size());
    for (const auto& [widget, cached] : mStyles) {
        compacted.push_back(mStyleStorage[cached.storageIndex]);
        styles.emplace(widget, CachedStyle{compacted.size() - 1, cached.lifetime, cached.contextRevision});
    }
    mStyleStorage.swap(compacted);
    mStyles.swap(styles);
}

void StylePass::endTraversal() {
    llassert(mTraversalDepth != 0);
    if (mTraversalDepth) --mTraversalDepth;
    mTree.endTraversal();
}

const Style& StylePass::style(const Widget& widget) {
    if (mInvalidated) {
        mStyles.clear();
        mInvalidated = false;
    }
    const auto found = mStyles.find(&widget);
    const auto lifetime = widget.lifetime().lock();
    if (found != mStyles.end() && found->second.lifetime.lock() == lifetime && found->second.contextRevision == widget.styleContextRevision())
        return mStyleStorage[found->second.storageIndex];
    if (found != mStyles.end()) mStyles.erase(found);

    const ConstWidgetVisit widgetSnapshot(widget);
    const std::weak_ptr<char> widgetLifetime = widget.lifetime();
    const WidgetRef<const Widget> styledRef(&widget);
    const Widget* parent = widgetSnapshot.parent;
    const std::uint64_t contextRevision = widget.styleContextRevision();
    const Widget* owner = &widget;
    Style resolved;
    if (widget.part().empty()) resolved = mStyleSheet.resolveWidget(widget);
    else {
        for (const Widget* candidate = widget.parent(); candidate; candidate = candidate->parent()) {
            if (candidate->styleElement() != widget.styleElement()) continue;
            owner = candidate;
            if (candidate->part().empty()) break;
        }
        resolved = mStyleSheet.resolveWidgetPart(*owner, widget);
    }
    widget.constrainResolvedStyle(resolved);
    const Widget* current = styledRef.get();
    const auto transient = [&]() -> const Style& {
        mStyleStorage.emplace_back(std::move(resolved));
        return mStyleStorage.back();
    };
    if (!current || !widgetSnapshot.styleValid()) return transient();
    if (parent) {
        const ConstWidgetVisit parentSnapshot(*parent);
        const Style& parentStyle = style(*parent);
        current = styledRef.get();
        const Widget* currentParent = parentSnapshot.get();
        if (!current || !currentParent || !widgetSnapshot.styleValid() || !parentSnapshot.styleValid()) return transient();
        inheritStyle(resolved, parentStyle);
    }

    current = styledRef.get();
    if (!current || !widgetSnapshot.styleValid()) return transient();
    const std::uint64_t finalContextRevision = current->styleContextRevision();
    mStyleStorage.emplace_back(std::move(resolved));
    const std::size_t storageIndex = mStyleStorage.size() - 1;
    mStyles[&widget] =
        CachedStyle{storageIndex, widgetLifetime, finalContextRevision == contextRevision ? contextRevision : finalContextRevision};
    return mStyleStorage[storageIndex];
}

StylePass::ChildSnapshot StylePass::orderedChildren(const Widget& parent) {
    return mTree.orderedChildren(parent, [this](const Widget& node) -> const Style& { return style(node); });
}

StylePass::ChildSnapshot StylePass::sourceChildren(const Widget& parent) {
    return mTree.sourceChildren(parent);
}
} // namespace radia::ui
