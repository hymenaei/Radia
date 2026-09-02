/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "dom/node.h"

namespace radia::ui {
class Fragment final : public Node {
public:
    Fragment();
    ~Fragment() override;

    Fragment(const Fragment&) = delete;
    Fragment& operator=(const Fragment&) = delete;
    Fragment(Fragment&&) = delete;
    Fragment& operator=(Fragment&&) = delete;

    Fragment* asFragment() noexcept override { return this; }
    const Fragment* asFragment() const noexcept override { return this; }

    Node* firstChild() noexcept override;
    const Node* firstChild() const noexcept override;
    Node* lastChild() noexcept override;
    const Node* lastChild() const noexcept override;
    NodeList childNodes() override;
    ConstNodeList childNodes() const override;

    Node* append(NodePtr child);
    Node* append(FragmentPtr fragment);
    Node* prepend(NodePtr child);
    Node* prepend(FragmentPtr fragment);
    void replaceChildren();
    void replaceChildren(FragmentPtr fragment);

private:
    friend class Document;
    friend class detail::NodeMutation;

    std::vector<NodePtr> mChildren;
};
} // namespace radia::ui
