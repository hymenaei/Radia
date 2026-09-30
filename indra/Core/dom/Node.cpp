/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "Node.h"
#include <algorithm>
#include <iterator>
#include "Element.h"
#include "Fragment.h"
#include "NodeMutation.h"

namespace Core {
using detail::NodeMutation;

namespace {
enum class SiblingDirection {
    Previous,
    Next
};

template<typename NodeType> NodeType* siblingIn(const std::vector<NodePtr>& children, NodeType* node, SiblingDirection direction) {
    const auto found = std::find_if(children.begin(), children.end(), [node](const NodePtr& child) {
        return child.get() == node;
    });
    if (found == children.end())
        return nullptr;
    if (direction == SiblingDirection::Previous)
        return found == children.begin() ? nullptr : std::prev(found)->get();
    const auto next = std::next(found);
    return next == children.end() ? nullptr : next->get();
}
} // namespace

Node* Node::previousSibling() noexcept {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    if (Element* element = parent->asElement())
        return siblingIn(element->mChildren, this, SiblingDirection::Previous);
    if (Fragment* fragment = parent->asFragment())
        return siblingIn(fragment->mChildren, this, SiblingDirection::Previous);
    return nullptr;
}

const Node* Node::previousSibling() const noexcept {
    const Node* parent = parentNode();
    if (!parent)
        return nullptr;
    if (const Element* element = parent->asElement())
        return siblingIn(element->mChildren, this, SiblingDirection::Previous);
    if (const Fragment* fragment = parent->asFragment())
        return siblingIn(fragment->mChildren, this, SiblingDirection::Previous);
    return nullptr;
}

Node* Node::nextSibling() noexcept {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    if (Element* element = parent->asElement())
        return siblingIn(element->mChildren, this, SiblingDirection::Next);
    if (Fragment* fragment = parent->asFragment())
        return siblingIn(fragment->mChildren, this, SiblingDirection::Next);
    return nullptr;
}

const Node* Node::nextSibling() const noexcept {
    const Node* parent = parentNode();
    if (!parent)
        return nullptr;
    if (const Element* element = parent->asElement())
        return siblingIn(element->mChildren, this, SiblingDirection::Next);
    if (const Fragment* fragment = parent->asFragment())
        return siblingIn(fragment->mChildren, this, SiblingDirection::Next);
    return nullptr;
}

Node* Node::before(NodePtr node) {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    llassert_always(!parent->asDocument());
    if (Element* element = parent->asElement())
        return element->insertBefore(std::move(node), this);
    if (Fragment* fragment = parent->asFragment())
        return NodeMutation::insert(*fragment, std::move(node), this);
    return nullptr;
}

Node* Node::before(FragmentPtr fragment) {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    llassert_always(!parent->asDocument());
    if (Element* element = parent->asElement())
        return element->insertBefore(std::move(fragment), this);
    if (Fragment* parentFragment = parent->asFragment())
        return NodeMutation::insert(*parentFragment, std::move(fragment), this);
    return nullptr;
}

Node* Node::after(NodePtr node) {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    llassert_always(!parent->asDocument());
    Node* reference = nextSibling();
    if (Element* element = parent->asElement())
        return element->insertBefore(std::move(node), reference);
    if (Fragment* fragment = parent->asFragment())
        return NodeMutation::insert(*fragment, std::move(node), reference);
    return nullptr;
}

Node* Node::after(FragmentPtr fragment) {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    llassert_always(!parent->asDocument());
    Node* reference = nextSibling();
    if (Element* element = parent->asElement())
        return element->insertBefore(std::move(fragment), reference);
    if (Fragment* parentFragment = parent->asFragment())
        return NodeMutation::insert(*parentFragment, std::move(fragment), reference);
    return nullptr;
}

NodePtr Node::replaceWith(NodePtr node) {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    llassert_always(!parent->asDocument());
    if (Element* element = parent->asElement())
        return element->replaceNode(*this, std::move(node));
    if (Fragment* fragment = parent->asFragment())
        return NodeMutation::replace(*fragment, *this, std::move(node));
    return nullptr;
}

NodePtr Node::replaceWith(FragmentPtr fragment) {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    llassert_always(!parent->asDocument());
    if (Element* element = parent->asElement())
        return element->replaceNode(*this, std::move(fragment));
    if (Fragment* parentFragment = parent->asFragment())
        return NodeMutation::replace(*parentFragment, *this, std::move(fragment));
    return nullptr;
}

NodePtr Node::remove() {
    Node* parent = parentNode();
    if (!parent)
        return nullptr;
    if (Element* element = parent->asElement())
        return element->removeNode(*this);
    if (Fragment* fragment = parent->asFragment())
        return NodeMutation::remove(*fragment, *this);
    if (Document* document = parent->asDocument())
        return NodeMutation::remove(*document, *this);
    return nullptr;
}
} // namespace Core
