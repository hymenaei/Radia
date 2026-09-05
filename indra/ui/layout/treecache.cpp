/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "layout/treecache.h"

namespace radia::ui {
void TreeTraversalCache::beginTraversal() {
    if (mTraversalDepth++ != 0) return;
    if (mResetAtBoundary) {
        mSource.clear();
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

TreeTraversalCache::ChildSnapshot TreeTraversalCache::build(Element& parent) {
    SnapshotCache& cache = mSource;
    SnapshotCache& activeCache = mActiveSource;
    const auto lifetime = detail::NodeAccess::lifetime(parent).lock();
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

    const ConstElementVisit parentState(parent);
    auto result = std::make_shared<std::vector<ElementRef<Element>>>();
    const ElementList children = parent.children();
    result->reserve(children.size());
    for (Element* child : children) result->emplace_back(child);

    const auto commit = [&] {
        if (mResetAtBoundary) {
            if (active()) {
                activeCache.snapshots[&parent] = result;
                activeCache.lifetimes[&parent] = detail::NodeAccess::lifetime(parent);
                activeCache.revisions[&parent] = revision;
            }
            return;
        }
        cache.snapshots[&parent] = result;
        cache.lifetimes[&parent] = detail::NodeAccess::lifetime(parent);
        cache.revisions[&parent] = revision;
    };

    if (!parentState.layoutValid()) return std::make_shared<std::vector<ElementRef<Element>>>();
    commit();
    return result;
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::sourceChildren(Element& parent) {
    return build(parent);
}
} // namespace radia::ui
