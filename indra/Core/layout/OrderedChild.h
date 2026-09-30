/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <vector>
#include "Element.h"
#include "ElementInternal.h"
#include "PseudoElement.h"

namespace Core::Layout {
struct OrderedChildRef {
    Core::detail::NodeRef node;
    Style::PseudoElement* pseudoElement = nullptr;

    OrderedChildRef() = default;
    explicit OrderedChildRef(Node* child)
        : node(child) {}
    explicit OrderedChildRef(Style::PseudoElement* child)
        : pseudoElement(child) {}

    bool isPseudoElement() const noexcept { return pseudoElement != nullptr; }
    bool attachedTo(const Element& parent) const noexcept {
        return pseudoElement ? pseudoElement->parentPseudoElement() == nullptr && &pseudoElement->originatingElement() == &parent
                             : node && node.get()->parentElement() == &parent;
    }
    bool attachedTo(const Style::PseudoElement& parent) const noexcept {
        return pseudoElement && pseudoElement->parentPseudoElement() == &parent;
    }
    Node* get() const noexcept { return node.get(); }
    Element* element() const noexcept { return node.element(); }
    Text* text() const noexcept { return node.text(); }
    explicit operator bool() const noexcept { return pseudoElement || static_cast<bool>(node); }
};

using OrderedChildSnapshot = std::shared_ptr<const std::vector<OrderedChildRef>>;
} // namespace Core::Layout
