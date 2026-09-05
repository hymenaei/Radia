/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <memory>
#include <vector>

namespace radia::ui {
class Document;
class Element;
class Fragment;
class Text;
class Node;
enum class NodeType : uint8_t { Document, Element, Fragment, Text };

using NodePtr = std::unique_ptr<Node>;
using ElementPtr = std::unique_ptr<Element>;
using FragmentPtr = std::unique_ptr<Fragment>;
using NodeList = std::vector<Node*>;
using ConstNodeList = std::vector<const Node*>;
using ElementList = std::vector<Element*>;
using ConstElementList = std::vector<const Element*>;

namespace detail {
struct DocumentIdentity;
class NodeAccess;
class NodeMutation;
} // namespace detail

class Node {
public:
    virtual ~Node() = default;

    NodeType nodeType() const noexcept { return mNodeType; }
    Node* parentNode() noexcept { return mParentNode; }
    const Node* parentNode() const noexcept { return mParentNode; }
    Element* parentElement() noexcept { return mParent; }
    const Element* parentElement() const noexcept { return mParent; }
    virtual Node* firstChild() noexcept { return nullptr; }
    virtual const Node* firstChild() const noexcept { return nullptr; }
    virtual Node* lastChild() noexcept { return nullptr; }
    virtual const Node* lastChild() const noexcept { return nullptr; }
    virtual NodeList childNodes() { return {}; }
    virtual ConstNodeList childNodes() const { return {}; }
    Node* previousSibling() noexcept;
    const Node* previousSibling() const noexcept;
    Node* nextSibling() noexcept;
    const Node* nextSibling() const noexcept;
    Node* before(NodePtr node);
    Node* before(FragmentPtr fragment);
    Node* after(NodePtr node);
    Node* after(FragmentPtr fragment);
    NodePtr replaceWith(NodePtr node);
    NodePtr replaceWith(FragmentPtr fragment);
    NodePtr remove();
    virtual Document* asDocument() noexcept { return nullptr; }
    virtual const Document* asDocument() const noexcept { return nullptr; }
    virtual Element* asElement() noexcept { return nullptr; }
    virtual const Element* asElement() const noexcept { return nullptr; }
    virtual Fragment* asFragment() noexcept { return nullptr; }
    virtual const Fragment* asFragment() const noexcept { return nullptr; }
    virtual Text* asText() noexcept { return nullptr; }
    virtual const Text* asText() const noexcept { return nullptr; }

protected:
    explicit Node(NodeType nodeType) : mNodeType(nodeType) {}

    Element* mParent = nullptr;
    Node* mParentNode = nullptr;
    bool mFlowBreakBefore = false;

private:
    friend class detail::NodeAccess;

    NodeType mNodeType;
    std::shared_ptr<char> mLifetime = std::make_shared<char>(0);
    std::shared_ptr<detail::DocumentIdentity> mDocumentIdentity;
};
} // namespace radia::ui
