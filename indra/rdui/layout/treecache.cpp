/**
 * @file treecache.cpp
 * @brief Implements cached source/order child snapshots.
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
#include "layout/treecache.h"
#include <algorithm>
#include "style/style.h"

namespace rdui {
void TreeTraversalCache::beginTraversal() {
    if (mTraversalDepth++ != 0) return;
    if (mResetAtBoundary) {
        mOrdered.clear();
        mSource.clear();
        mActiveOrdered.clear();
        mActiveSource.clear();
        mResetAtBoundary = false;
    }
}

void TreeTraversalCache::endTraversal() {
    llassert(mTraversalDepth != 0);
    if (mTraversalDepth) --mTraversalDepth;
}

void TreeTraversalCache::invalidateOrdering() {
    mResetAtBoundary = true;
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::build(const Widget& parent, bool ordered, const StyleResolver* resolve) {
    SnapshotCache& cache = ordered ? mOrdered : mSource;
    SnapshotCache& active_cache = ordered ? mActiveOrdered : mActiveSource;
    const auto lifetime = parent.lifetime().lock();
    const std::uint64_t revision = parent.mChildSnapshotRevision;
    const auto found = cache.snapshots.find(&parent);
    const auto cached_lifetime = cache.lifetimes.find(&parent);
    const auto active_found = active_cache.snapshots.find(&parent);
    const auto active_lifetime = active_cache.lifetimes.find(&parent);
    const auto cached_revision = cache.revisions.find(&parent);
    if (mResetAtBoundary
        && active()
        && active_found != active_cache.snapshots.end()
        && active_lifetime != active_cache.lifetimes.end()
        && active_lifetime->second.lock() == lifetime)
        return active_found->second;
    if (!mResetAtBoundary
        && found != cache.snapshots.end()
        && cached_lifetime != cache.lifetimes.end()
        && cached_revision != cache.revisions.end()
        && cached_lifetime->second.lock() == lifetime
        && cached_revision->second == revision)
        return found->second;

    const ConstWidgetVisit parentState(parent);
    auto result = std::make_shared<std::vector<WidgetRef<Widget>>>();
    result->reserve(parent.children().size());
    for (const auto& child : parent.children())
        if (!ordered || child->visibility() != Visibility::Collapsed) result->emplace_back(child.get());

    const auto commit = [&] {
        if (mResetAtBoundary) {
            if (active()) {
                active_cache.snapshots[&parent] = result;
                active_cache.lifetimes[&parent] = parent.lifetime();
                active_cache.revisions[&parent] = revision;
            }
            return;
        }
        cache.snapshots[&parent] = result;
        cache.lifetimes[&parent] = parent.lifetime();
        cache.revisions[&parent] = revision;
    };

    if (!parentState.valid()) return std::make_shared<std::vector<WidgetRef<Widget>>>();
    if (ordered && resolve) {
        const Style& parentStyle = (*resolve)(parent);
        if (!parentState.styleValid()) return std::make_shared<std::vector<WidgetRef<Widget>>>();
        if (parentStyle.flow == Flow::Row || parentStyle.flow == Flow::Column) {
            std::vector<std::pair<WidgetRef<Widget>, int>> ranked;
            ranked.reserve(result->size());
            for (const WidgetRef<Widget>& child_ref : *result) {
                Widget* child = child_ref.get();
                if (!child || child->parent() != &parent) continue;
                const WidgetVisit child_state(*child);
                const Style& child_style = (*resolve)(*child);
                child = child_state.get();
                if (!parentState.styleValid() || !child_state.styleValid() || !child || child->parent() != &parent)
                    return std::make_shared<std::vector<WidgetRef<Widget>>>();
                ranked.emplace_back(child_ref, child_style.order);
            }
            std::stable_sort(ranked.begin(), ranked.end(), [](const auto& left, const auto& right) { return left.second < right.second; });
            result->clear();
            result->reserve(ranked.size());
            for (auto& [child_ref, order] : ranked) result->push_back(std::move(child_ref));
        }
        if (!parentState.styleValid()) return std::make_shared<std::vector<WidgetRef<Widget>>>();
    }
    if (!parentState.valid()) return std::make_shared<std::vector<WidgetRef<Widget>>>();
    commit();
    return result;
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::orderedChildren(const Widget& parent, const StyleResolver& resolve) {
    return build(parent, true, &resolve);
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::sourceChildren(const Widget& parent) {
    return build(parent, false, nullptr);
}
} // namespace rdui
