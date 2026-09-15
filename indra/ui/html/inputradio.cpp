/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <functional>
#include <vector>
#include "dom/elementinternal.h"
#include "html/input.h"

namespace radia::ui {
namespace {
using detail::ElementInternalAccess;
using detail::NodeRef;

Node* treeRoot(Node& node) {
    Node* root = &node;
    while (root->parentNode()) root = root->parentNode();
    return root;
}

struct RadioTraversalState {
    RadioTraversalState(HTMLInputElement& currentInput, Node& rootNode)
        : current(&currentInput), root(&rootNode), rootPointer(&rootNode), parent(currentInput.parentNode()),
          mountEpoch(ElementInternalAccess::mountEpoch(currentInput)) {}

    bool valid() const {
        HTMLInputElement* input = current.get();
        Node* rootNode = root.get();
        return input
            && rootNode == rootPointer
            && treeRoot(*input) == rootPointer
            && input->parentNode() == parent
            && ElementInternalAccess::mountEpoch(*input) == mountEpoch;
    }

    ElementRef<HTMLInputElement> current;
    NodeRef root;
    Node* rootPointer = nullptr;
    Node* parent = nullptr;
    detail::MountEpoch mountEpoch;
};

bool visitInputs(Node& node, const std::function<bool(HTMLInputElement&)>& visitor) {
    std::vector<NodeRef> children;
    for (Node* child : node.childNodes()) children.emplace_back(child);

    if (Element* element = node.asElement())
        if (auto* input = dynamic_cast<HTMLInputElement*>(element))
            if (!visitor(*input)) return false;
    for (const NodeRef& childRef : children)
        if (Node* child = childRef.get(); child && !visitInputs(*child, visitor)) return false;
    return true;
}
} // namespace

void HTMLInputElement::activateRadio() {
    activateChecked(true);
}

void HTMLInputElement::setCheckedFromRadioGroup(bool checked) {
    if (!isRadioType() || this->checked() == checked) return;
    mValueState.value = checked;
    updateCheckedState(checked);
    notifyValueState();
}

void HTMLInputElement::updateRadioGroup() {
    if (!isRadioType()) {
        refreshIndeterminateState();
        return;
    }

    Node* root = treeRoot(*this);
    const RadioTraversalState traversal(*this, *root);
    if (checked()) {
        visitInputs(*root, [&traversal, this](HTMLInputElement& candidate) {
            if (!traversal.valid()) return false;
            if (&candidate == this || !candidate.isRadioType() || mName.empty() || candidate.mName != mName) return true;
            candidate.setCheckedFromRadioGroup(false);
            return traversal.valid();
        });
    }
    if (!traversal.valid()) return;
    refreshRadioGroup();
}

void HTMLInputElement::refreshRadioGroup() {
    if (!isRadioType()) {
        refreshIndeterminateState();
        return;
    }

    if (mName.empty()) {
        updateIndeterminateState(false);
        return;
    }
    refreshRadioGroup(mName);
}

void HTMLInputElement::refreshRadioGroup(std::string_view groupName, const HTMLInputElement* excluded) {
    if (groupName.empty()) return;
    Node* root = treeRoot(*this);
    const RadioTraversalState traversal(*this, *root);
    bool groupHasChecked = false;
    if (!visitInputs(*root,
                     [&traversal, groupName, excluded, &groupHasChecked](HTMLInputElement& candidate) {
                         if (!traversal.valid()) return false;
                         if (&candidate == excluded || !candidate.isRadioType() || candidate.mName != groupName) return true;
                         groupHasChecked = groupHasChecked || candidate.checked();
                         return true;
                     })
        || !traversal.valid())
        return;

    visitInputs(*root, [&traversal, groupName, excluded, groupHasChecked](HTMLInputElement& candidate) {
        if (!traversal.valid()) return false;
        if (&candidate == excluded || !candidate.isRadioType() || candidate.mName != groupName) return true;
        candidate.updateIndeterminateState(!groupHasChecked);
        return true;
    });
}
} // namespace radia::ui
