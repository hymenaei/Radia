/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/button.h"
#include "html/elementnames.h"
#include "render/paintcontext.h"
#include "resource/elementdefinition.h"
#include "style/computedstyle.h"

namespace radia::ui {
HTMLButtonElement::HTMLButtonElement() : HTMLButtonElement(kButtonTag.localName) {}

HTMLButtonElement::HTMLButtonElement(std::string_view elementName) : HTMLElement(elementName) {}

void HTMLButtonElement::constrainResolvedStyle(ComputedStyle& style) const {
    style.alignContentBlockCenter = style.appearance == AppearanceMode::Auto && style.display == DisplayMode::InlineBlock;
}

void HTMLButtonElement::paint(PaintContext& context, const ComputedStyle& style, float scale) const {
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

ResourceElementDefinition detail::ElementDefinitions::button() {
    return defineElement<HTMLButtonElement>(kButtonTag.localName).build();
}
} // namespace radia::ui
