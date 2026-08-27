/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/icon.h"
#include "elements/elementdefinition.h"
#include "render/paintcontext.h"

namespace radia::ui {
namespace { constexpr char kElementName[] = "icon"; }

IconElement::IconElement(std::string name) : Element(kElementName), mName(std::move(name)) {}

IconElement& IconElement::setName(std::string name) {
    mName = std::move(name);
    invalidatePaint();
    return *this;
}

void IconElement::paint(PaintContext& context, const Style& style, float scale) const {
    context.paintBox(rect(), style);
    context.paintIcon(mName, rect(), style, scale);
}

ElementDefinition detail::ElementDefinitionFactory::icon() {
    return defineElement<IconElement>(kElementName).attributes({stringAttribute("src", &IconElement::setName)}).build();
}
} // namespace radia::ui
