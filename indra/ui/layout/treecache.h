/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <functional>
#include <memory>
#include <unordered_map>
#include <vector>
#include "elements/element.h"
#include "elements/elementinternal.h"

namespace radia::ui {
struct Style;

class TreeTraversalCache {
public:
    using ChildSnapshot = std::shared_ptr<const std::vector<ElementRef<Element>>>;
    using StyleResolver = std::function<const Style&(const Element&)>;

    void beginTraversal();
    void endTraversal();
    void invalidateOrdering();
    bool active() const { return mTraversalDepth != 0; }

    ChildSnapshot orderedChildren(Element& parent, const StyleResolver& resolve);
    ChildSnapshot sourceChildren(Element& parent);

private:
    struct SnapshotCache {
        std::unordered_map<const Element*, ChildSnapshot> snapshots;
        std::unordered_map<const Element*, std::weak_ptr<char>> lifetimes;
        std::unordered_map<const Element*, std::uint64_t> revisions;

        void clear() {
            snapshots.clear();
            lifetimes.clear();
            revisions.clear();
        }
    };

    ChildSnapshot build(Element& parent, bool ordered, const StyleResolver* resolve);

    SnapshotCache mOrdered;
    SnapshotCache mSource;
    SnapshotCache mActiveOrdered;
    SnapshotCache mActiveSource;
    std::size_t mTraversalDepth = 0;
    bool mResetAtBoundary = false;
};
} // namespace radia::ui
