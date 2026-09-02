/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/fragment.h"
#include "dom/mutation.h"
#include "html/elementfactory.h"
#include "llerror.h"

namespace radia::ui {
using detail::DocumentIdentity;
using detail::findElementInTree;
using detail::HTMLElementFactory;
using detail::NodeAccess;
using detail::NodeMutation;

Document::Document(ElementPtr documentElement) : Node(NodeType::Document), mIdentity(std::make_shared<DocumentIdentity>()) {
    llassert_always(documentElement);
    NodeAccess::setDocumentIdentity(*this, mIdentity);
    NodeMutation::adopt(*this, *documentElement);
    NodeAccess::setParent(*documentElement, this);
    mChildren.emplace_back(std::move(documentElement));
    if (Element* root = mChildren.front()->asElement()) root->notifyTreeAttached();
}

Document::~Document() {
    mDestroying = true;
    auto observers = std::move(mDestructionObservers);
    for (auto& observer : observers)
        if (observer) observer();
}

void Document::addDestructionObserver(std::function<void()> observer) {
    llassert_always(observer);
    if (!mDestroying) mDestructionObservers.emplace_back(std::move(observer));
}

ElementPtr Document::releaseDocumentElement() {
    if (mChildren.empty()) return nullptr;
    NodePtr node = NodeMutation::remove(*this, *mChildren.front());
    return ElementPtr(static_cast<Element*>(node.release()));
}

ElementPtr Document::createElement(std::string_view elementName) const {
    ElementPtr element = HTMLElementFactory::Create(elementName);
    if (!element) LL_ERRS("UI") << "Unknown UI Element type: " << elementName << LL_ENDL;
    NodeMutation::adopt(*this, *element);
    return element;
}

FragmentPtr Document::createFragment() const {
    auto fragment = std::make_unique<Fragment>();
    NodeMutation::adopt(*this, *fragment);
    return fragment;
}

NodePtr Document::adoptNode(NodePtr node) const {
    llassert_always(node);
    NodeMutation::adopt(*this, *node);
    return node;
}

Element* Document::documentElement() noexcept {
    return mChildren.empty() ? nullptr : mChildren.front()->asElement();
}

const Element* Document::documentElement() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.front()->asElement();
}

Node* Document::firstChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

const Node* Document::firstChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

Node* Document::lastChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.back().get();
}

const Node* Document::lastChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.back().get();
}

NodeList Document::childNodes() {
    NodeList result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren) result.push_back(child.get());
    return result;
}

ConstNodeList Document::childNodes() const {
    ConstNodeList result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren) result.push_back(child.get());
    return result;
}

Element* Document::getElementById(std::string_view id) noexcept {
    return id.empty() || !documentElement() ? nullptr : findElementInTree(*documentElement(), id);
}

const Element* Document::getElementById(std::string_view id) const noexcept {
    return id.empty() || !documentElement() ? nullptr : findElementInTree(*documentElement(), id);
}
} // namespace radia::ui
