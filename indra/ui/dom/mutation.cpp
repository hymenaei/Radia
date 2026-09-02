/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/mutation.h"
#include <algorithm>
#include <iterator>
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "dom/fragment.h"
#include "surface/surface.h"
#include "text/host.h"

namespace radia::ui::detail {
namespace { using NodeOwnerList = std::vector<NodePtr>; } // namespace

void NodeMutation::validateChild(const Node& parent, const Node* child) {
    llassert_always(child);
    llassert_always(child->nodeType() != NodeType::Document);
    llassert_always(child->nodeType() != NodeType::Fragment);
    llassert_always(!child->parentNode());
    if (const Element* element = child->asElement()) llassert_always(!element->mSurface);
    for (const Node* current = &parent; current; current = current->parentNode()) llassert_always(current != child);
}

void NodeMutation::adopt(const Document& document, Node& node) {
    adopt(node, NodeAccess::documentIdentity(document));
}

void NodeMutation::adopt(Node& node, const std::shared_ptr<DocumentIdentity>& identity) {
    llassert_always(node.nodeType() != NodeType::Document);
    llassert_always(!node.parentNode());
    if (Element* element = node.asElement()) llassert_always(!element->mSurface);
    assignDocumentIdentity(node, identity);
}

void NodeMutation::validateDetachedSubtree(const Node& node) {
    if (const Element* element = node.asElement()) {
        llassert_always(!element->mSurface);
        for (const Node& child : nodes(*element)) validateDetachedSubtree(child);
    } else if (const Fragment* fragment = node.asFragment()) {
        for (const NodePtr& child : fragment->mChildren) validateDetachedSubtree(*child);
    }
}

namespace {
void validateReference(const NodeOwnerList& children, Node* reference) {
    if (!reference) return;
    const auto found = std::find_if(children.begin(), children.end(), [reference](const NodePtr& node) { return node.get() == reference; });
    llassert_always(found != children.end());
}

NodeOwnerList::iterator findChild(NodeOwnerList& children, Node& child) {
    const auto found = std::find_if(children.begin(), children.end(), [&child](const NodePtr& node) { return node.get() == &child; });
    llassert_always(found != children.end());
    return found;
}

} // namespace

void NodeMutation::assignDocumentIdentity(Node& node, const std::shared_ptr<DocumentIdentity>& identity) {
    NodeAccess::setDocumentIdentity(node, identity);
    if (Element* element = node.asElement())
        for (Node& child : nodes(*element)) assignDocumentIdentity(child, identity);
    else if (Fragment* fragment = node.asFragment())
        for (const NodePtr& child : fragment->mChildren) assignDocumentIdentity(*child, identity);
}

void NodeMutation::validateFragment(const Node& parent, const Fragment& fragment) {
    for (const NodePtr& child : fragment.mChildren) {
        llassert_always(child);
        llassert_always(child->nodeType() != NodeType::Document);
        llassert_always(child->nodeType() != NodeType::Fragment);
        llassert_always(child->parentNode() == &fragment);
        validateDetachedSubtree(*child);
        for (const Node* current = &parent; current; current = current->parentNode()) llassert_always(current != child.get());
    }
}

void NodeMutation::clearTextSlots(Element& parent) {
    if (!parent.mSuppressTextSlots) parent.mTextContentSlots.clear();
}

void NodeMutation::notifyAncestorAdded(Element& parent, Element& child) {
    std::vector<ElementRef<Element>> ancestors;
    for (Element* ancestor = parent.parentElement(); ancestor; ancestor = ancestor->parentElement()) ancestors.emplace_back(ancestor);
    const ElementRef<Element> childLifetime(&child);
    for (const ElementRef<Element>& ancestorRef : ancestors) {
        Element* ancestor = ancestorRef.get();
        Element* currentChild = childLifetime.get();
        if (!ancestor || !currentChild) return;
        ancestor->onDescendantAdded(*currentChild);
    }
}

void NodeMutation::notifyAncestorRemoved(Element& parent, Element& child) {
    std::vector<ElementRef<Element>> ancestors;
    for (Element* ancestor = parent.parentElement(); ancestor; ancestor = ancestor->parentElement()) ancestors.emplace_back(ancestor);
    const ElementRef<Element> childLifetime(&child);
    for (const ElementRef<Element>& ancestorRef : ancestors) {
        Element* ancestor = ancestorRef.get();
        Element* currentChild = childLifetime.get();
        if (!ancestor || !currentChild) return;
        ancestor->onDescendantRemoved(*currentChild);
    }
}

void NodeMutation::detachElementChild(Element& parent, Node& node, Surface* surface) {
    Element* child = node.asElement();
    const ElementRef<Element> parentLifetime(&parent);
    const ElementRef<Element> childLifetime(child);

    NodeAccess::setParent(node, nullptr);
    if (!child) return;

    if (parentLifetime) parent.onChildRemoved(*child);
    if (childLifetime) child->notifyTreeDetached();
    if (parentLifetime) notifyAncestorRemoved(parent, *child);
    if (surface && childLifetime) surface->elementBecameUnavailable(*child);
    if (childLifetime) child->setSurface(nullptr);
}

void NodeMutation::detachOrphanedChild(Node& node, Surface* surface) {
    NodeAccess::setParent(node, nullptr);
    Element* child = node.asElement();
    if (!child) return;

    const ElementRef<Element> childLifetime(child);
    if (childLifetime) child->notifyTreeDetached();
    if (surface && childLifetime) surface->elementBecameUnavailable(*child);
    if (childLifetime) child->setSurface(nullptr);
}

void NodeMutation::attachElementChild(Element& parent, Node& node) {
    NodeAccess::setParent(node, &parent);
}

void NodeMutation::detachAll(Element& parent) {
    clearTextSlots(parent);
    Surface* surface = parent.mSurface;
    const ElementRef<Element> parentLifetime(&parent);
    NodeOwners children = std::move(parent.mChildren);
    parent.mChildren.clear();
    ++parent.mChildSnapshotRevision;
    ++parent.mChildTopologyRevision;
    if (surface) surface->invalidateOrderingCache();
    parent.invalidateMeasure();
    for (NodePtr& child : children) {
        if (!parentLifetime) {
            detachOrphanedChild(*child, surface);
            continue;
        }
        detachElementChild(parent, *child, surface);
    }
    if (!parentLifetime) return;
    parent.onChildrenCleared();
}

void NodeMutation::detachAll(Fragment& parent) {
    NodeOwners children = std::move(parent.mChildren);
    parent.mChildren.clear();
    for (NodePtr& child : children) NodeAccess::setParent(*child, nullptr);
}

Node* NodeMutation::insertElementChild(Element& parent, NodePtr child, Node* reference) {
    validateChild(parent, child.get());
    NodeOwners children;
    children.emplace_back(std::move(child));
    return insertElementChildren(parent, std::move(children), reference);
}

Node* NodeMutation::insertElementChildren(Element& parent, NodeOwners children, Node* reference) {
    if (children.empty()) return nullptr;
    validateReference(parent.mChildren, reference);

    const ElementRef<Element> parentLifetime(&parent);
    const NodeRef firstLifetime(children.front().get());
    const std::size_t referenceIndex = reference
        ? static_cast<std::size_t>(
              std::find_if(parent.mChildren.begin(), parent.mChildren.end(), [reference](const NodePtr& node) { return node.get() == reference; })
              - parent.mChildren.begin())
        : parent.mChildren.size();
    for (NodePtr& child : children) {
        llassert_always(child && !child->parentNode());
        adopt(*child, NodeAccess::documentIdentity(parent));
        attachElementChild(parent, *child);
    }

    std::vector<ElementRef<Element>> elementRefs;
    elementRefs.reserve(children.size());
    std::size_t insertionIndex = referenceIndex;
    for (NodePtr& child : children) {
        Node* added = child.get();
        parent.mChildren.insert(parent.mChildren.begin() + static_cast<std::ptrdiff_t>(insertionIndex), std::move(child));
        ++insertionIndex;
        if (Element* element = added->asElement()) elementRefs.emplace_back(element);
    }
    ++parent.mChildSnapshotRevision;
    ++parent.mChildTopologyRevision;
    if (parent.mSurface) parent.mSurface->invalidateOrderingCache();
    if (!parent.mSuppressTextSlots) parent.mTextContentSlots.clear();
    parent.invalidateMeasure();
    for (const ElementRef<Element>& childRef : elementRefs) {
        if (Element* child = childRef.get()) child->setSurface(parent.mSurface);
        if (!parentLifetime) return firstLifetime.get();
    }

    for (const ElementRef<Element>& childRef : elementRefs) {
        Element* currentParent = parentLifetime.get();
        if (!currentParent) return firstLifetime.get();
        Element* child = childRef.get();
        if (!child || child->parentElement() != currentParent) continue;
        child->notifyTreeAttached();
        currentParent = parentLifetime.get();
        if (!currentParent) return firstLifetime.get();
        child = childRef.get();
        if (child && child->parentElement() == currentParent) {
            currentParent->onChildAdded(*child);
            currentParent = parentLifetime.get();
            child = childRef.get();
            if (currentParent && child && child->parentElement() == currentParent) notifyAncestorAdded(*currentParent, *child);
        }
    }
    return firstLifetime.get();
}

Node* NodeMutation::insertFragmentChild(Fragment& parent, NodePtr child, Node* reference) {
    validateChild(parent, child.get());
    NodeOwners children;
    children.emplace_back(std::move(child));
    return insertFragmentChildren(parent, std::move(children), reference);
}

Node* NodeMutation::insertFragmentChildren(Fragment& parent, NodeOwners children, Node* reference) {
    if (children.empty()) return nullptr;
    validateReference(parent.mChildren, reference);
    const std::size_t referenceIndex = reference
        ? static_cast<std::size_t>(
              std::find_if(parent.mChildren.begin(), parent.mChildren.end(), [reference](const NodePtr& node) { return node.get() == reference; })
              - parent.mChildren.begin())
        : parent.mChildren.size();
    Node* first = children.front().get();
    std::size_t insertionIndex = referenceIndex;
    for (NodePtr& child : children) {
        llassert_always(child && !child->parentNode());
        adopt(*child, NodeAccess::documentIdentity(parent));
        NodeAccess::setParent(*child, &parent);
        parent.mChildren.insert(parent.mChildren.begin() + static_cast<std::ptrdiff_t>(insertionIndex), std::move(child));
        ++insertionIndex;
    }
    return first;
}

Node* NodeMutation::insertElementFragment(Element& parent, FragmentPtr fragment, Node* reference) {
    llassert_always(fragment);
    validateReference(parent.mChildren, reference);
    validateFragment(parent, *fragment);
    NodeOwners children = std::move(fragment->mChildren);
    fragment->mChildren.clear();
    for (NodePtr& child : children) NodeAccess::setParent(*child, nullptr);
    return insertElementChildren(parent, std::move(children), reference);
}

Node* NodeMutation::insertFragmentFragment(Fragment& parent, FragmentPtr fragment, Node* reference) {
    llassert_always(fragment);
    validateReference(parent.mChildren, reference);
    validateFragment(parent, *fragment);
    NodeOwners children = std::move(fragment->mChildren);
    fragment->mChildren.clear();
    for (NodePtr& child : children) NodeAccess::setParent(*child, nullptr);
    return insertFragmentChildren(parent, std::move(children), reference);
}

NodePtr NodeMutation::replaceElementChild(Element& parent, Node& child, FragmentPtr replacement) {
    llassert_always(replacement);
    validateFragment(parent, *replacement);
    auto found = findChild(parent.mChildren, child);
    const auto next = std::next(found);
    const NodeRef referenceLifetime(next == parent.mChildren.end() ? nullptr : next->get());
    const bool flowBreakBefore = NodeAccess::flowBreakBefore(child);
    const ElementRef<Element> parentLifetime(&parent);
    NodePtr detached = std::move(*found);
    parent.mChildren.erase(found);
    ++parent.mChildSnapshotRevision;
    ++parent.mChildTopologyRevision;
    if (parent.mSurface) parent.mSurface->invalidateOrderingCache();
    clearTextSlots(parent);
    parent.invalidateMeasure();
    detachElementChild(parent, *detached, parent.mSurface);
    if (!parentLifetime) return detached;

    NodeOwners children = std::move(replacement->mChildren);
    replacement->mChildren.clear();
    for (NodePtr& node : children) NodeAccess::setParent(*node, nullptr);
    if (children.empty()) {
        parent.invalidateMeasure();
        return detached;
    }
    NodeAccess::setFlowBreakBefore(*children.front(), flowBreakBefore);
    Node* reference = referenceLifetime.get();
    if (!reference || reference->parentNode() != &parent) reference = nullptr;
    insertElementChildren(parent, std::move(children), reference);
    return detached;
}

NodePtr NodeMutation::replaceFragmentChild(Fragment& parent, Node& child, FragmentPtr replacement) {
    llassert_always(replacement);
    validateFragment(parent, *replacement);
    auto found = findChild(parent.mChildren, child);
    const auto next = std::next(found);
    const NodeRef referenceLifetime(next == parent.mChildren.end() ? nullptr : next->get());
    const bool flowBreakBefore = NodeAccess::flowBreakBefore(child);
    NodePtr detached = std::move(*found);
    parent.mChildren.erase(found);
    NodeAccess::setParent(*detached, nullptr);
    NodeOwners children = std::move(replacement->mChildren);
    replacement->mChildren.clear();
    for (NodePtr& node : children) NodeAccess::setParent(*node, nullptr);
    if (!children.empty()) {
        NodeAccess::setFlowBreakBefore(*children.front(), flowBreakBefore);
        insertFragmentChildren(parent, std::move(children), referenceLifetime.get());
    }
    return detached;
}

Node* NodeMutation::replaceElementRange(Element& parent, Node& first, Node& last, FragmentPtr replacement) {
    llassert_always(replacement);
    validateFragment(parent, *replacement);
    auto firstFound = findChild(parent.mChildren, first);
    auto lastFound = findChild(parent.mChildren, last);
    llassert_always(firstFound <= lastFound);
    const auto next = std::next(lastFound);
    const NodeRef referenceLifetime(next == parent.mChildren.end() ? nullptr : next->get());
    const bool flowBreakBefore = NodeAccess::flowBreakBefore(first);
    const ElementRef<Element> parentLifetime(&parent);
    Surface* surface = parent.mSurface;

    NodeOwners removed;
    for (auto current = firstFound; current != lastFound + 1; ++current) removed.push_back(std::move(*current));
    parent.mChildren.erase(firstFound, lastFound + 1);
    ++parent.mChildSnapshotRevision;
    ++parent.mChildTopologyRevision;
    if (surface) surface->invalidateOrderingCache();
    clearTextSlots(parent);
    parent.invalidateMeasure();
    for (NodePtr& node : removed) {
        if (!parentLifetime) {
            detachOrphanedChild(*node, surface);
            continue;
        }
        detachElementChild(parent, *node, surface);
    }
    if (!parentLifetime) return nullptr;

    NodeOwners children = std::move(replacement->mChildren);
    replacement->mChildren.clear();
    for (NodePtr& node : children) NodeAccess::setParent(*node, nullptr);
    if (children.empty()) {
        parent.invalidateMeasure();
        return nullptr;
    }
    NodeAccess::setFlowBreakBefore(*children.front(), flowBreakBefore);
    Node* reference = referenceLifetime.get();
    if (!reference || reference->parentNode() != &parent) reference = nullptr;
    return insertElementChildren(parent, std::move(children), reference);
}

Node* NodeMutation::insert(Element& parent, NodePtr child, Node* reference) {
    return insertElementChild(parent, std::move(child), reference);
}

Node* NodeMutation::insert(Element& parent, FragmentPtr fragment, Node* reference) {
    return insertElementFragment(parent, std::move(fragment), reference);
}

Node* NodeMutation::insert(Fragment& parent, NodePtr child, Node* reference) {
    return insertFragmentChild(parent, std::move(child), reference);
}

Node* NodeMutation::insert(Fragment& parent, FragmentPtr fragment, Node* reference) {
    return insertFragmentFragment(parent, std::move(fragment), reference);
}

void NodeMutation::replaceChildren(Element& parent) {
    detachAll(parent);
}

Node* NodeMutation::replaceChildren(Element& parent, NodePtr child) {
    validateChild(parent, child.get());
    const ElementRef<Element> parentLifetime(&parent);
    detachAll(parent);
    if (!parentLifetime) return nullptr;
    NodeOwners children;
    children.emplace_back(std::move(child));
    return insertElementChildren(parent, std::move(children), nullptr);
}

void NodeMutation::replaceChildren(Element& parent, FragmentPtr fragment) {
    llassert_always(fragment);
    validateFragment(parent, *fragment);
    const ElementRef<Element> parentLifetime(&parent);
    detachAll(parent);
    if (!parentLifetime) return;
    insertElementFragment(parent, std::move(fragment), nullptr);
}

void NodeMutation::replaceChildren(Fragment& parent) {
    detachAll(parent);
}

void NodeMutation::replaceChildren(Fragment& parent, FragmentPtr fragment) {
    llassert_always(fragment);
    validateFragment(parent, *fragment);
    detachAll(parent);
    insertFragmentFragment(parent, std::move(fragment), nullptr);
}

NodePtr NodeMutation::replace(Element& parent, Node& child, NodePtr replacement) {
    validateChild(parent, replacement.get());
    auto found = findChild(parent.mChildren, child);
    const auto next = std::next(found);
    const NodeRef referenceLifetime(next == parent.mChildren.end() ? nullptr : next->get());
    const bool flowBreakBefore = NodeAccess::flowBreakBefore(child);
    const ElementRef<Element> parentLifetime(&parent);
    NodePtr detached = std::move(*found);
    parent.mChildren.erase(found);
    ++parent.mChildSnapshotRevision;
    ++parent.mChildTopologyRevision;
    if (parent.mSurface) parent.mSurface->invalidateOrderingCache();
    clearTextSlots(parent);
    parent.invalidateMeasure();
    detachElementChild(parent, *detached, parent.mSurface);
    if (!parentLifetime) return detached;
    Node* reference = referenceLifetime.get();
    if (!reference || reference->parentNode() != &parent) reference = nullptr;
    Node* inserted = insertElementChild(parent, std::move(replacement), reference);
    if (inserted) NodeAccess::setFlowBreakBefore(*inserted, flowBreakBefore);
    return detached;
}

NodePtr NodeMutation::replace(Element& parent, Node& child, FragmentPtr replacement) {
    return replaceElementChild(parent, child, std::move(replacement));
}

NodePtr NodeMutation::replace(Fragment& parent, Node& child, NodePtr replacement) {
    validateChild(parent, replacement.get());
    auto found = findChild(parent.mChildren, child);
    const std::size_t index = static_cast<std::size_t>(found - parent.mChildren.begin());
    const bool flowBreakBefore = NodeAccess::flowBreakBefore(child);
    NodePtr detached = std::move(*found);
    parent.mChildren.erase(found);
    NodeAccess::setParent(*detached, nullptr);
    NodeAccess::setParent(*replacement, nullptr);
    Node* inserted = insertFragmentChild(parent, std::move(replacement), index < parent.mChildren.size() ? parent.mChildren[index].get() : nullptr);
    if (inserted) NodeAccess::setFlowBreakBefore(*inserted, flowBreakBefore);
    return detached;
}

NodePtr NodeMutation::replace(Fragment& parent, Node& child, FragmentPtr replacement) {
    return replaceFragmentChild(parent, child, std::move(replacement));
}

Node* NodeMutation::replaceRange(Element& parent, Node& first, Node& last, FragmentPtr replacement) {
    return replaceElementRange(parent, first, last, std::move(replacement));
}

NodePtr NodeMutation::remove(Element& parent, Node& child) {
    auto found = findChild(parent.mChildren, child);
    const ElementRef<Element> parentLifetime(&parent);
    NodePtr detached = std::move(*found);
    parent.mChildren.erase(found);
    ++parent.mChildSnapshotRevision;
    ++parent.mChildTopologyRevision;
    if (parent.mSurface) parent.mSurface->invalidateOrderingCache();
    clearTextSlots(parent);
    parent.invalidateMeasure();
    detachElementChild(parent, *detached, parent.mSurface);
    if (!parentLifetime) return detached;
    return detached;
}

NodePtr NodeMutation::remove(Fragment& parent, Node& child) {
    auto found = findChild(parent.mChildren, child);
    NodePtr detached = std::move(*found);
    parent.mChildren.erase(found);
    NodeAccess::setParent(*detached, nullptr);
    return detached;
}

NodePtr NodeMutation::remove(Document& parent, Node& child) {
    llassert_always(parent.mChildren.size() == 1U && parent.mChildren.front().get() == &child);
    Element* element = child.asElement();
    llassert_always(element);

    NodePtr detached = std::move(parent.mChildren.front());
    parent.mChildren.clear();
    NodeAccess::setParent(*detached, nullptr);
    if (element->mSurface) llassert_always(element->mSurface->unmountBorrowed(*element));
    element->notifyTreeDetached();
    return detached;
}

void NodeMutation::setTextData(Text& text, std::string value) {
    if (text.mValue == value) return;
    text.mValue = std::move(value);
    text.mLayout->setText(text.mValue);
    if (Element* owner = text.parentElement()) owner->invalidateText();
}
} // namespace radia::ui::detail
