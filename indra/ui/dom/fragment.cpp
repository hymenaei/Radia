/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/fragment.h"
#include <utility>
#include "dom/mutation.h"

namespace radia::ui {
using detail::NodeMutation;

Fragment::Fragment() : Node(NodeType::Fragment) {}
Fragment::~Fragment() = default;

Node* Fragment::firstChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

const Node* Fragment::firstChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

Node* Fragment::lastChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.back().get();
}

const Node* Fragment::lastChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.back().get();
}

NodeList Fragment::childNodes() {
    NodeList result;
    result.reserve(mChildren.size());
    for (const NodePtr& child : mChildren) result.push_back(child.get());
    return result;
}

ConstNodeList Fragment::childNodes() const {
    ConstNodeList result;
    result.reserve(mChildren.size());
    for (const NodePtr& child : mChildren) result.push_back(child.get());
    return result;
}

Node* Fragment::append(NodePtr child) {
    return NodeMutation::insert(*this, std::move(child), nullptr);
}

Node* Fragment::append(FragmentPtr fragment) {
    return NodeMutation::insert(*this, std::move(fragment), nullptr);
}

Node* Fragment::prepend(NodePtr child) {
    return NodeMutation::insert(*this, std::move(child), firstChild());
}

Node* Fragment::prepend(FragmentPtr fragment) {
    return NodeMutation::insert(*this, std::move(fragment), firstChild());
}

void Fragment::replaceChildren() {
    NodeMutation::replaceChildren(*this);
}

void Fragment::replaceChildren(FragmentPtr fragment) {
    NodeMutation::replaceChildren(*this, std::move(fragment));
}

} // namespace radia::ui
