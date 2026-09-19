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
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "style/pseudoelement.h"

namespace radia::ui {
struct OrderedChildRef {
    detail::NodeRef node;
    PseudoElement* pseudoElement = nullptr;

    OrderedChildRef() = default;
    explicit OrderedChildRef(Node* child) : node(child) {}
    explicit OrderedChildRef(PseudoElement* child) : pseudoElement(child) {}

    bool isPseudoElement() const noexcept { return pseudoElement != nullptr; }
    bool attachedTo(const Element& parent) const noexcept {
        return pseudoElement ? pseudoElement->parentPseudoElement() == nullptr && &pseudoElement->originatingElement() == &parent
                             : node && node.get()->parentElement() == &parent;
    }
    bool attachedTo(const PseudoElement& parent) const noexcept { return pseudoElement && pseudoElement->parentPseudoElement() == &parent; }
    Node* get() const noexcept { return node.get(); }
    Element* element() const noexcept { return node.element(); }
    Text* text() const noexcept { return node.text(); }
    explicit operator bool() const noexcept { return pseudoElement || static_cast<bool>(node); }
};

using OrderedChildSnapshot = std::shared_ptr<const std::vector<OrderedChildRef>>;

class TreeTraversalCache {
public:
    using ChildSnapshot = std::shared_ptr<const std::vector<ElementRef<Element>>>;

    void beginTraversal();
    void endTraversal();
    void invalidateOrdering();
    bool active() const { return mTraversalDepth != 0; }

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

    ChildSnapshot build(Element& parent);

    SnapshotCache mSource;
    SnapshotCache mActiveSource;
    std::size_t mTraversalDepth = 0;
    bool mResetAtBoundary = false;
};
} // namespace radia::ui
