/**
 * @file treecache.h
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

#ifndef RD_LAYOUT_TREECACHE_H
#define RD_LAYOUT_TREECACHE_H

#include <cstdint>
#include <functional>
#include <memory>
#include <unordered_map>
#include <vector>
#include "widgets/widget.h"

namespace radia::ui {
struct Style;

class TreeTraversalCache {
public:
    using ChildSnapshot = std::shared_ptr<const std::vector<WidgetRef<Widget>>>;
    using StyleResolver = std::function<const Style&(const Widget&)>;

    void beginTraversal();
    void endTraversal();
    void invalidateOrdering();
    bool active() const { return mTraversalDepth != 0; }

    ChildSnapshot orderedChildren(const Widget& parent, const StyleResolver& resolve);
    ChildSnapshot sourceChildren(const Widget& parent);

private:
    struct SnapshotCache {
        std::unordered_map<const Widget*, ChildSnapshot> snapshots;
        std::unordered_map<const Widget*, std::weak_ptr<char>> lifetimes;
        std::unordered_map<const Widget*, std::uint64_t> revisions;

        void clear() {
            snapshots.clear();
            lifetimes.clear();
            revisions.clear();
        }
    };

    ChildSnapshot build(const Widget& parent, bool ordered, const StyleResolver* resolve);

    SnapshotCache mOrdered;
    SnapshotCache mSource;
    SnapshotCache mActiveOrdered;
    SnapshotCache mActiveSource;
    std::size_t mTraversalDepth = 0;
    bool mResetAtBoundary = false;
};
} // namespace radia::ui
#endif // RD_LAYOUT_TREECACHE_H
