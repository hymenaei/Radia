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

namespace radia::ui {
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
    SnapshotCache& activeCache = ordered ? mActiveOrdered : mActiveSource;
    const auto lifetime = parent.lifetime().lock();
    const std::uint64_t revision = parent.mChildSnapshotRevision;
    const auto found = cache.snapshots.find(&parent);
    const auto cachedLifetime = cache.lifetimes.find(&parent);
    const auto activeFound = activeCache.snapshots.find(&parent);
    const auto activeLifetime = activeCache.lifetimes.find(&parent);
    const auto cachedRevision = cache.revisions.find(&parent);
    if (mResetAtBoundary
        && active()
        && activeFound != activeCache.snapshots.end()
        && activeLifetime != activeCache.lifetimes.end()
        && activeLifetime->second.lock() == lifetime)
        return activeFound->second;
    if (!mResetAtBoundary
        && found != cache.snapshots.end()
        && cachedLifetime != cache.lifetimes.end()
        && cachedRevision != cache.revisions.end()
        && cachedLifetime->second.lock() == lifetime
        && cachedRevision->second == revision)
        return found->second;

    const ConstWidgetVisit parentState(parent);
    auto result = std::make_shared<std::vector<WidgetRef<Widget>>>();
    result->reserve(parent.children().size());
    for (const auto& child : parent.children()) result->emplace_back(child.get());

    const auto commit = [&] {
        if (mResetAtBoundary) {
            if (active()) {
                activeCache.snapshots[&parent] = result;
                activeCache.lifetimes[&parent] = parent.lifetime();
                activeCache.revisions[&parent] = revision;
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
        if (parentStyle.display == DisplayMode::Flex) {
            std::vector<std::pair<WidgetRef<Widget>, int>> ranked;
            ranked.reserve(result->size());
            for (const WidgetRef<Widget>& childRef : *result) {
                Widget* child = childRef.get();
                if (!child || child->parent() != &parent) continue;
                const WidgetVisit childState(*child);
                const Style& childStyle = (*resolve)(*child);
                child = childState.get();
                if (!parentState.styleValid() || !childState.styleValid() || !child || child->parent() != &parent)
                    return std::make_shared<std::vector<WidgetRef<Widget>>>();
                ranked.emplace_back(childRef, childStyle.order);
            }
            std::stable_sort(ranked.begin(), ranked.end(), [](const auto& left, const auto& right) { return left.second < right.second; });
            result->clear();
            result->reserve(ranked.size());
            for (auto& [childRef, order] : ranked) result->push_back(std::move(childRef));
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
} // namespace radia::ui
