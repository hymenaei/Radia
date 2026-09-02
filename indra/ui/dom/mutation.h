/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "dom/element.h"

namespace radia::ui {
class Fragment;

namespace detail {
class NodeMutation final {
public:
    static Node* insert(Element& parent, NodePtr child, Node* reference);
    static Node* insert(Element& parent, FragmentPtr fragment, Node* reference);
    static Node* insert(Fragment& parent, NodePtr child, Node* reference);
    static Node* insert(Fragment& parent, FragmentPtr fragment, Node* reference);

    static void replaceChildren(Element& parent);
    static Node* replaceChildren(Element& parent, NodePtr child);
    static void replaceChildren(Element& parent, FragmentPtr fragment);
    static void replaceChildren(Fragment& parent);
    static void replaceChildren(Fragment& parent, FragmentPtr fragment);

    static NodePtr replace(Element& parent, Node& child, NodePtr replacement);
    static NodePtr replace(Element& parent, Node& child, FragmentPtr replacement);
    static NodePtr replace(Fragment& parent, Node& child, NodePtr replacement);
    static NodePtr replace(Fragment& parent, Node& child, FragmentPtr replacement);
    static Node* replaceRange(Element& parent, Node& first, Node& last, FragmentPtr replacement);

    static NodePtr remove(Element& parent, Node& child);
    static NodePtr remove(Fragment& parent, Node& child);
    static NodePtr remove(Document& parent, Node& child);
    static void setTextData(Text& text, std::string value);

private:
    friend class radia::ui::Document;

    using NodeOwners = std::vector<NodePtr>;

    static void adopt(const Document& document, Node& node);
    static void adopt(Node& node, const std::shared_ptr<DocumentIdentity>& identity);
    static void validateChild(const Node& parent, const Node* child);
    static void validateDetachedSubtree(const Node& node);
    static void assignDocumentIdentity(Node& node, const std::shared_ptr<DocumentIdentity>& identity);
    static void validateFragment(const Node& parent, const Fragment& fragment);
    static void clearTextSlots(Element& parent);
    static void detachElementChild(Element& parent, Node& node, Surface* surface);
    static void detachOrphanedChild(Node& node, Surface* surface);
    static void notifyAncestorAdded(Element& parent, Element& child);
    static void notifyAncestorRemoved(Element& parent, Element& child);
    static void attachElementChild(Element& parent, Node& node);
    static void detachAll(Element& parent);
    static void detachAll(Fragment& parent);
    static Node* insertElementChild(Element& parent, NodePtr child, Node* reference);
    static Node* insertElementChildren(Element& parent, NodeOwners children, Node* reference);
    static Node* insertFragmentChild(Fragment& parent, NodePtr child, Node* reference);
    static Node* insertFragmentChildren(Fragment& parent, NodeOwners children, Node* reference);
    static Node* insertElementFragment(Element& parent, FragmentPtr fragment, Node* reference);
    static Node* insertFragmentFragment(Fragment& parent, FragmentPtr fragment, Node* reference);
    static NodePtr replaceElementChild(Element& parent, Node& child, FragmentPtr replacement);
    static NodePtr replaceFragmentChild(Fragment& parent, Node& child, FragmentPtr replacement);
    static Node* replaceElementRange(Element& parent, Node& first, Node& last, FragmentPtr replacement);
};
} // namespace detail
} // namespace radia::ui
