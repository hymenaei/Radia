/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <functional>
#include "html/input.h"

namespace radia::ui {
namespace {
Node* treeRoot(Node& node) {
    Node* root = &node;
    while (root->parentNode()) root = root->parentNode();
    return root;
}

void visitInputs(Node& node, const std::function<void(HTMLInputElement&)>& visitor) {
    if (Element* element = node.asElement())
        if (auto* input = dynamic_cast<HTMLInputElement*>(element)) visitor(*input);
    for (Node* child : node.childNodes())
        if (child) visitInputs(*child, visitor);
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
    if (checked()) {
        visitInputs(*root, [this](HTMLInputElement& candidate) {
            if (&candidate == this || !candidate.isRadioType() || mName.empty() || candidate.mName != mName) return;
            candidate.setCheckedFromRadioGroup(false);
        });
    }
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
    bool groupHasChecked = false;
    visitInputs(*root, [groupName, excluded, &groupHasChecked](HTMLInputElement& candidate) {
        if (&candidate == excluded || !candidate.isRadioType() || candidate.mName != groupName) return;
        groupHasChecked = groupHasChecked || candidate.checked();
    });

    visitInputs(*root, [groupName, excluded, groupHasChecked](HTMLInputElement& candidate) {
        if (&candidate == excluded || !candidate.isRadioType() || candidate.mName != groupName) return;
        candidate.updateIndeterminateState(!groupHasChecked);
    });
}
} // namespace radia::ui
