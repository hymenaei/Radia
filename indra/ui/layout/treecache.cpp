/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
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

TreeTraversalCache::ChildSnapshot TreeTraversalCache::build(Element& parent, bool ordered, const StyleResolver* resolve) {
    SnapshotCache& cache = ordered ? mOrdered : mSource;
    SnapshotCache& activeCache = ordered ? mActiveOrdered : mActiveSource;
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

    if (!parentState.valid()) return std::make_shared<std::vector<ElementRef<Element>>>();
    if (ordered && resolve) {
        const Style& parentStyle = (*resolve)(parent);
        if (!parentState.styleValid()) return std::make_shared<std::vector<ElementRef<Element>>>();
        if (parentStyle.display == DisplayMode::Flex) {
            std::vector<std::pair<ElementRef<Element>, int>> ranked;
            ranked.reserve(result->size());
            for (const ElementRef<Element>& childRef : *result) {
                Element* child = childRef.get();
                if (!child || child->parentElement() != &parent) continue;
                const ElementVisit childState(*child);
                const Style& childStyle = (*resolve)(*child);
                child = childState.get();
                if (!parentState.styleValid() || !childState.styleValid() || !child || child->parentElement() != &parent)
                    return std::make_shared<std::vector<ElementRef<Element>>>();
                ranked.emplace_back(childRef, childStyle.order);
            }
            std::stable_sort(ranked.begin(), ranked.end(), [](const auto& left, const auto& right) { return left.second < right.second; });
            result->clear();
            result->reserve(ranked.size());
            for (auto& [childRef, order] : ranked) result->push_back(std::move(childRef));
        }
        if (!parentState.styleValid()) return std::make_shared<std::vector<ElementRef<Element>>>();
    }
    if (!parentState.valid()) return std::make_shared<std::vector<ElementRef<Element>>>();
    commit();
    return result;
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::orderedChildren(Element& parent, const StyleResolver& resolve) {
    return build(parent, true, &resolve);
}

TreeTraversalCache::ChildSnapshot TreeTraversalCache::sourceChildren(Element& parent) {
    return build(parent, false, nullptr);
}
} // namespace radia::ui
