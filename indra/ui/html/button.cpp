/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/button.h"
#include "ComputedStyleProperties.h"
#include "html/elementnames.h"
#include "paint/paintcontext.h"
#include "resource/elementdefinition.h"
#include "style/computedstyle.h"

namespace radia::ui {
HTMLButtonElement::HTMLButtonElement() : HTMLButtonElement(HTMLTagName(HTMLTag::Button)) {}

HTMLButtonElement::HTMLButtonElement(std::string_view elementName) : HTMLElement(elementName) {}

AccessibleSemantics HTMLButtonElement::accessibleSemantics() const {
    AccessibleSemantics result = HTMLElement::accessibleSemantics();
    result.role = AccessibleRole::Button;
    return result;
}

void HTMLButtonElement::constrainResolvedStyle(ComputedStyle& style) const {
    style.alignContentBlockCenter = style.appearance() == Appearance::Auto && style.display() == Display::InlineBlock;
}

void HTMLButtonElement::paint(PaintContext& context, const ComputedStyle& style, float scale) const {
    if (style.appearance() == Appearance::Auto) {
        NativeButtonPaintRequest request;
        request.bounds = rect();
        request.style = style;
        request.disabled = disabled();
        request.hovered = hovered();
        request.pressed = active();
        request.focused = focused();
        request.focusVisible = focusVisible();
        request.scale = scale;
        context.paintNativeButton(request);
        return;
    }
    Element::paint(context, style, scale);
}

ResourceElementDefinition detail::ElementDefinitions::button() {
    return defineElement<HTMLButtonElement>(HTMLTagName(HTMLTag::Button))
        .attributes({allowedAttribute("type")})
        .validate([](const ElementBuildInput& input, HTMLButtonElement& button, ElementBuildContext& context) {
            const ElementAttribute* type = input.find("type");
            if (!type) return;
            if (!type->hasValue) {
                context.error("layout.button.type_value_required", "Button type requires a value.", input.sourceName, type->source.begin.line,
                              type->source.begin.column);
                return;
            }
            const std::string typeName = canonicalizeHTMLName(type->value);
            if (typeName == "submit" || typeName == "reset") {
                context.error("layout.button.type_unsupported", "Button type is not supported: " + type->value + ".", input.sourceName,
                              type->source.begin.line, type->source.begin.column);
                return;
            }
            if (typeName != "button") {
                context.error("layout.button.type_invalid", "Unsupported button type: " + type->value + ".", input.sourceName,
                              type->source.begin.line, type->source.begin.column);
                return;
            }
            button.setAttribute("type", type->value);
        })
        .labelable()
        .build();
}
} // namespace radia::ui
