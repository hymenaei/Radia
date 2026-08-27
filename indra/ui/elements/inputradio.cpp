/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <functional>
#include "elements/input.h"
#include "render/paintcontext.h"
#include "style/style.h"

namespace radia::ui {
namespace {
constexpr float kRadioSize = 13.f;

Node* treeRoot(Node& node) {
    Node* root = &node;
    while (root->parentNode()) root = root->parentNode();
    return root;
}

void visitInputs(Node& node, const std::function<void(InputElement&)>& visitor) {
    if (Element* element = node.asElement())
        if (auto* input = dynamic_cast<InputElement*>(element)) visitor(*input);
    for (Node* child : node.childNodes())
        if (child) visitInputs(*child, visitor);
}
} // namespace

Vec2 InputElement::nativeRadioIntrinsicSize() const {
    return {kRadioSize, kRadioSize};
}

void InputElement::paintNativeRadio(PaintContext& context, const Style& style, float) const {
    const Rect bounds = rect();
    context.paintBox(bounds, nativeControlStyle(style, nativeControlFill(false, disabled()), nativeControlBorder(disabled()), bounds.h * .5f));
    if (!checked()) return;

    const float inset = std::max(2.f, std::min(bounds.w, bounds.h) * .3f);
    const float size = std::max(0.f, std::min(bounds.w, bounds.h) - inset * 2.f);
    const Rect dot{bounds.x + (bounds.w - size) * .5f, bounds.y + (bounds.h - size) * .5f, size, size};
    context.paintBox(dot, nativeMarkStyle(style, nativeControlFill(true, disabled()), size * .5f));
}

void InputElement::activateRadio() {
    activateChecked(true);
}

void InputElement::setCheckedFromRadioGroup(bool checked) {
    if (!isRadioType() || this->checked() == checked) return;
    mValueState.value = checked;
    updateCheckedState(checked);
    notifyValueState();
}

void InputElement::updateRadioGroup() {
    if (!isRadioType()) {
        refreshIndeterminateState();
        return;
    }

    Node* root = treeRoot(*this);
    if (checked()) {
        visitInputs(*root, [this](InputElement& candidate) {
            if (&candidate == this || !candidate.isRadioType() || mName.empty() || candidate.mName != mName) return;
            candidate.setCheckedFromRadioGroup(false);
        });
    }
    refreshRadioGroup();
}

void InputElement::refreshRadioGroup() {
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

void InputElement::refreshRadioGroup(std::string_view groupName, const InputElement* excluded) {
    if (groupName.empty()) return;
    Node* root = treeRoot(*this);
    bool groupHasChecked = false;
    visitInputs(*root, [groupName, excluded, &groupHasChecked](InputElement& candidate) {
        if (&candidate == excluded || !candidate.isRadioType() || candidate.mName != groupName) return;
        groupHasChecked = groupHasChecked || candidate.checked();
    });

    visitInputs(*root, [groupName, excluded, groupHasChecked](InputElement& candidate) {
        if (&candidate == excluded || !candidate.isRadioType() || candidate.mName != groupName) return;
        candidate.updateIndeterminateState(!groupHasChecked);
    });
}
} // namespace radia::ui
