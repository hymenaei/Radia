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

namespace rdui {
namespace {
LayoutContextKey makeContextKey(const StyleSheet& style_sheet, const TextMetrics& text_metrics) {
    return {&style_sheet, &text_metrics, style_sheet.generation(), text_metrics.generation()};
}
} // namespace

StylePass::StylePass(const StyleSheet& style_sheet, const TextMetrics& text_metrics)
    : mStyleSheet(style_sheet), mTextMetrics(text_metrics), mContext(makeContextKey(style_sheet, text_metrics)) {}

bool StylePass::matches(const StyleSheet& style_sheet, const TextMetrics& text_metrics) const {
    return mContext == makeContextKey(style_sheet, text_metrics);
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
    constexpr std::size_t storage_slack = 32;
    if (mStyleStorage.size() <= mStyles.size() * 2 + storage_slack) return;

    std::deque<Style> compacted;
    std::unordered_map<const Widget*, CachedStyle> styles;
    styles.reserve(mStyles.size());
    for (const auto& [widget, cached] : mStyles) {
        compacted.push_back(mStyleStorage[cached.storage_index]);
        styles.emplace(widget, CachedStyle{compacted.size() - 1, cached.lifetime, cached.context_revision});
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
    if (found != mStyles.end()
        && found->second.lifetime.lock() == lifetime
        && found->second.context_revision == widget.styleContextRevision())
        return mStyleStorage[found->second.storage_index];
    if (found != mStyles.end()) mStyles.erase(found);

    const ConstWidgetVisit widget_state(widget);
    const std::weak_ptr<char> widget_lifetime = widget.lifetime();
    const WidgetRef<const Widget> styled_lifetime(&widget);
    const Widget* parent = widget_state.parent;
    const std::uint64_t context_revision = widget.styleContextRevision();
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
    const Widget* current = styled_lifetime.get();
    const auto transient = [&]() -> const Style& {
        mStyleStorage.emplace_back(std::move(resolved));
        return mStyleStorage.back();
    };
    if (!current || !widget_state.styleValid())
        return transient();
    if (parent) {
        const ConstWidgetVisit parent_state(*parent);
        const Style& parent_style = style(*parent);
        current = styled_lifetime.get();
        const Widget* current_parent = parent_state.get();
        if (!current || !current_parent || !widget_state.styleValid() || !parent_state.styleValid())
            return transient();
        inheritStyle(resolved, parent_style);
    }

    current = styled_lifetime.get();
    if (!current || !widget_state.styleValid())
        return transient();
    const std::uint64_t final_context_revision = current->styleContextRevision();
    mStyleStorage.emplace_back(std::move(resolved));
    const std::size_t storage_index = mStyleStorage.size() - 1;
    mStyles[&widget] = CachedStyle{
        storage_index, widget_lifetime, final_context_revision == context_revision ? context_revision : final_context_revision};
    return mStyleStorage[storage_index];
}

StylePass::ChildSnapshot StylePass::orderedChildren(const Widget& parent) {
    return mTree.orderedChildren(parent, [this](const Widget& node) -> const Style& { return style(node); });
}

StylePass::ChildSnapshot StylePass::sourceChildren(const Widget& parent) {
    return mTree.sourceChildren(parent);
}
} // namespace rdui
