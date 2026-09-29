/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "TreeTraversalCache.h"

namespace Core::Layout {
void TreeTraversalCache::beginTraversal() {
    if (mTraversalDepth++ != 0)
        return;
    if (mResetAtBoundary) {
        mSource.clear();
        mActiveSource.clear();
        mResetAtBoundary = false;
    }
}

void TreeTraversalCache::endTraversal() {
    llassert(mTraversalDepth != 0);
    if (mTraversalDepth)
        --mTraversalDepth;
}

void TreeTraversalCache::invalidateOrdering() { mResetAtBoundary = true; }

TreeTraversalCache::ChildSnapshot TreeTraversalCache::build(Element& parent) {
    SnapshotCache& cache = mSource;
    SnapshotCache& activeCache = mActiveSource;
    const auto lifetime = Core::detail::NodeAccess::lifetime(parent).lock();
    const std::uint64_t revision = parent.mChildSnapshotRevision;
    const auto found = cache.entries.find(&parent);
    const auto activeFound = activeCache.entries.find(&parent);
    if (mResetAtBoundary && active() && activeFound != activeCache.entries.end() && activeFound->second.lifetime.lock() == lifetime)
        return activeFound->second.snapshot;
    if (!mResetAtBoundary && found != cache.entries.end() && found->second.lifetime.lock() == lifetime
        && found->second.revision == revision)
        return found->second.snapshot;

    const ConstElementVisit parentState(parent);
    auto result = std::make_shared<std::vector<ElementRef<Element>>>();
    const ElementList children = parent.children();
    result->reserve(children.size());
    for (Element* child : children)
        result->emplace_back(child);

    if (!parentState.layoutValid())
        return std::make_shared<std::vector<ElementRef<Element>>>();

    const SnapshotCacheEntry entry {result, Core::detail::NodeAccess::lifetime(parent), revision};
    if (mResetAtBoundary) {
        if (active())
            activeCache.entries[&parent] = entry;
    } else {
        cache.entries[&parent] = entry;
    }
    return result;
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::sourceChildren(Element& parent) { return build(parent); }
} // namespace Core::Layout
