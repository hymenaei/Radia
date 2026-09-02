/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/icon.h"
#include "html/elementnames.h"
#include "render/paintcontext.h"
#include "resource/elementdefinition.h"

namespace radia::ui {
HTMLIconElement::HTMLIconElement(std::string name) : HTMLElement(kIconTag.localName), mName(std::move(name)) {}

HTMLIconElement& HTMLIconElement::setName(std::string name) {
    mName = std::move(name);
    if (mName.empty()) removeAttribute("src");
    else setAttribute("src", mName);
    invalidatePaint();
    return *this;
}

void HTMLIconElement::paint(PaintContext& context, const Style& style, float scale) const {
    context.paintBox(rect(), style);
    context.paintIcon(mName, rect(), style, scale);
}

ResourceElementDefinition detail::ElementDefinitions::icon() {
    return defineElement<HTMLIconElement>(kIconTag.localName).attributes({stringAttribute("src", &HTMLIconElement::setName)}).build();
}
} // namespace radia::ui
