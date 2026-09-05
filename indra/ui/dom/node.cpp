/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/node.h"
#include <algorithm>
#include <iterator>
#include "dom/element.h"
#include "dom/fragment.h"
#include "dom/mutation.h"

namespace radia::ui {
using detail::NodeMutation;

Node* Node::previousSibling() noexcept {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    const NodeList siblings = parent->childNodes();
    const auto found = std::find(siblings.begin(), siblings.end(), this);
    if (found == siblings.end() || found == siblings.begin()) return nullptr;
    return *std::prev(found);
}

const Node* Node::previousSibling() const noexcept {
    const Node* parent = parentNode();
    if (!parent) return nullptr;
    const ConstNodeList siblings = parent->childNodes();
    const auto found = std::find(siblings.begin(), siblings.end(), this);
    if (found == siblings.end() || found == siblings.begin()) return nullptr;
    return *std::prev(found);
}

Node* Node::nextSibling() noexcept {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    const NodeList siblings = parent->childNodes();
    const auto found = std::find(siblings.begin(), siblings.end(), this);
    if (found == siblings.end()) return nullptr;
    const auto next = std::next(found);
    return next == siblings.end() ? nullptr : *next;
}

const Node* Node::nextSibling() const noexcept {
    const Node* parent = parentNode();
    if (!parent) return nullptr;
    const ConstNodeList siblings = parent->childNodes();
    const auto found = std::find(siblings.begin(), siblings.end(), this);
    if (found == siblings.end()) return nullptr;
    const auto next = std::next(found);
    return next == siblings.end() ? nullptr : *next;
}

Node* Node::before(NodePtr node) {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    if (Element* element = parent->asElement()) return element->insertBefore(std::move(node), this);
    if (Fragment* fragment = parent->asFragment()) return NodeMutation::insert(*fragment, std::move(node), this);
    return nullptr;
}

Node* Node::before(FragmentPtr fragment) {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    if (Element* element = parent->asElement()) return element->insertBefore(std::move(fragment), this);
    if (Fragment* parentFragment = parent->asFragment()) return NodeMutation::insert(*parentFragment, std::move(fragment), this);
    return nullptr;
}

Node* Node::after(NodePtr node) {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    Node* reference = nextSibling();
    if (Element* element = parent->asElement()) return element->insertBefore(std::move(node), reference);
    if (Fragment* fragment = parent->asFragment()) return NodeMutation::insert(*fragment, std::move(node), reference);
    return nullptr;
}

Node* Node::after(FragmentPtr fragment) {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    Node* reference = nextSibling();
    if (Element* element = parent->asElement()) return element->insertBefore(std::move(fragment), reference);
    if (Fragment* parentFragment = parent->asFragment()) return NodeMutation::insert(*parentFragment, std::move(fragment), reference);
    return nullptr;
}

NodePtr Node::replaceWith(NodePtr node) {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    if (Element* element = parent->asElement()) return element->replaceNode(*this, std::move(node));
    if (Fragment* fragment = parent->asFragment()) return NodeMutation::replace(*fragment, *this, std::move(node));
    return nullptr;
}

NodePtr Node::replaceWith(FragmentPtr fragment) {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    if (Element* element = parent->asElement()) return element->replaceNode(*this, std::move(fragment));
    if (Fragment* parentFragment = parent->asFragment()) return NodeMutation::replace(*parentFragment, *this, std::move(fragment));
    return nullptr;
}

NodePtr Node::remove() {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    if (Element* element = parent->asElement()) return element->removeNode(*this);
    if (Fragment* fragment = parent->asFragment()) return NodeMutation::remove(*fragment, *this);
    if (Document* document = parent->asDocument()) return NodeMutation::remove(*document, *this);
    return nullptr;
}
} // namespace radia::ui
