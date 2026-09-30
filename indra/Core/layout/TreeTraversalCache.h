/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <memory>
#include <unordered_map>
#include <vector>
#include "Element.h"
#include "ElementInternal.h"

namespace Core::Layout {
class TreeTraversalCache {
public:
    using ChildSnapshot = std::shared_ptr<const std::vector<ElementRef<Element>>>;

    void beginTraversal();
    void endTraversal();
    void invalidateOrdering();
    bool active() const { return mTraversalDepth != 0; }

    ChildSnapshot sourceChildren(Element& parent);

private:
    struct SnapshotCacheEntry {
        ChildSnapshot snapshot;
        std::weak_ptr<char> lifetime;
        std::uint64_t revision = 0;
    };

    struct SnapshotCache {
        std::unordered_map<const Element*, SnapshotCacheEntry> entries;

        void clear() { entries.clear(); }
    };

    ChildSnapshot build(Element& parent);

    SnapshotCache mSource;
    SnapshotCache mActiveSource;
    std::size_t mTraversalDepth = 0;
    bool mResetAtBoundary = false;
};
} // namespace Core::Layout
