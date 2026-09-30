/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "Document.h"
#include "ElementInternal.h"
#include "Fragment.h"
#include "HTMLElementFactory.h"
#include "NodeMutation.h"

namespace Core {
using detail::findElementInTree;
using detail::HTMLElementFactory;
using detail::NodeAccess;
using detail::NodeMutation;

Document::Document(ElementPtr documentElement)
    : Node(NodeType::Document) {
    llassert_always(documentElement);
    NodeMutation::adopt(*documentElement, this);
    NodeAccess::setParent(*documentElement, this);
    mChildren.emplace_back(std::move(documentElement));
    if (Element* root = mChildren.front()->asElement())
        root->notifyTreeAttached();
}

Document::~Document() {
    mDestroying = true;
    auto observers = std::move(mDestructionObservers);
    for (auto& observer : observers)
        if (observer)
            observer();
}

void Document::addDestructionObserver(std::function<void()> observer) {
    llassert_always(observer);
    if (!mDestroying)
        mDestructionObservers.emplace_back(std::move(observer));
}

ElementPtr Document::releaseDocumentElement() {
    if (mChildren.empty())
        return nullptr;
    NodePtr node = NodeMutation::remove(*this, *mChildren.front());
    return ElementPtr(static_cast<Element*>(node.release()));
}

ElementPtr Document::createElement(std::string_view elementName) const {
    ElementPtr element = HTMLElementFactory::create(elementName);
    if (!element)
        return nullptr;
    NodeMutation::adopt(*element, const_cast<Document*>(this));
    return element;
}

FragmentPtr Document::createFragment() const {
    auto fragment = std::make_unique<Fragment>();
    NodeMutation::adopt(*fragment, const_cast<Document*>(this));
    return fragment;
}

NodePtr Document::adoptNode(NodePtr node) const {
    llassert_always(node);
    NodeMutation::adopt(*node, const_cast<Document*>(this));
    return node;
}

Element* Document::documentElement() noexcept { return mChildren.empty() ? nullptr : mChildren.front()->asElement(); }

const Element* Document::documentElement() const noexcept { return mChildren.empty() ? nullptr : mChildren.front()->asElement(); }

Node* Document::firstChild() noexcept { return mChildren.empty() ? nullptr : mChildren.front().get(); }

const Node* Document::firstChild() const noexcept { return mChildren.empty() ? nullptr : mChildren.front().get(); }

Node* Document::lastChild() noexcept { return mChildren.empty() ? nullptr : mChildren.back().get(); }

const Node* Document::lastChild() const noexcept { return mChildren.empty() ? nullptr : mChildren.back().get(); }

NodeSnapshot Document::childNodes() {
    NodeSnapshot result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren)
        result.push_back(child.get());
    return result;
}

ConstNodeSnapshot Document::childNodes() const {
    ConstNodeSnapshot result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren)
        result.push_back(child.get());
    return result;
}

Element* Document::getElementById(std::string_view id) noexcept {
    return id.empty() || !documentElement() ? nullptr : findElementInTree(*documentElement(), id);
}

const Element* Document::getElementById(std::string_view id) const noexcept {
    return id.empty() || !documentElement() ? nullptr : findElementInTree(*documentElement(), id);
}
} // namespace Core
