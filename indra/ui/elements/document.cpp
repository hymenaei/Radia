/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/document.h"
#include "elements/elementinternal.h"
#include "llerror.h"

namespace radia::ui {
namespace {
void assignIdentity(Node& node, const std::shared_ptr<detail::DocumentIdentity>& identity) {
    detail::NodeAccess::setDocumentIdentity(node, identity);
    if (Element* element = node.asElement())
        for (Node& child : detail::nodes(*element)) assignIdentity(child, identity);
}
} // namespace

Document::Document(ElementPtr documentElement) : Node(NodeType::Document), mIdentity(std::make_shared<detail::DocumentIdentity>()) {
    llassert_always(documentElement && !documentElement->parentNode());
    detail::NodeAccess::setDocumentIdentity(*this, mIdentity);
    detail::NodeAccess::setParent(*documentElement, this);
    assignIdentity(*documentElement, mIdentity);
    mChildren.emplace_back(std::move(documentElement));
    if (Element* root = mChildren.front()->asElement()) root->notifyTreeAttached();
}

Document::~Document() = default;

ElementPtr Document::releaseDocumentElement() {
    if (mChildren.empty()) return nullptr;
    NodePtr node = std::move(mChildren.front());
    mChildren.clear();
    if (Element* element = node->asElement()) element->notifyTreeDetached();
    detail::NodeAccess::setParent(*node, nullptr);
    detail::NodeAccess::setDocumentIdentity(*node, nullptr);
    return ElementPtr(static_cast<Element*>(node.release()));
}

ElementPtr Document::createElement(std::string_view elementName) const {
    ElementPtr element = detail::createRuntimeElement(elementName);
    if (!element) LL_ERRS("UI") << "Unknown UI Element type: " << elementName << LL_ENDL;
    assignIdentity(*element, mIdentity);
    return element;
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
    return documentElement() ? detail::findElementInScope(*documentElement(), id) : nullptr;
}

const Element* Document::getElementById(std::string_view id) const noexcept {
    return documentElement() ? detail::findElementInScope(*documentElement(), id) : nullptr;
}
} // namespace radia::ui
