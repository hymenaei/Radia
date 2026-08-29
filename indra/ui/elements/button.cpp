/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/button.h"
#include "elements/elementdefinition.h"
#include "render/paintcontext.h"
#include "style/style.h"

namespace radia::ui {
namespace { constexpr char kElementName[] = "button"; }

ButtonElement::ButtonElement() : ButtonElement(kElementName) {}

ButtonElement::ButtonElement(const char* elementName) : Element(elementName) {}

void ButtonElement::constrainResolvedStyle(Style& style) const {
    style.alignContentBlockCenter = style.appearance == AppearanceMode::Auto && style.display == DisplayMode::InlineBlock;
}

void ButtonElement::paint(PaintContext& context, const Style& style, float scale) const {
    if (style.appearance == AppearanceMode::Auto) {
        NativeButtonPaintRequest request;
        request.bounds = rect();
        request.style = style;
        request.disabled = disabled();
        request.hovered = hasState(ElementState::Hovered);
        request.pressed = hasState(ElementState::Active);
        request.focused = hasState(ElementState::Focused);
        request.focusVisible = hasState(ElementState::FocusVisible);
        request.scale = scale;
        context.paintNativeButton(request);
        return;
    }
    Element::paint(context, style, scale);
}

ElementDefinition detail::ElementDefinitionFactory::button() {
    return defineElement<ButtonElement>(kElementName).build();
}
} // namespace radia::ui
