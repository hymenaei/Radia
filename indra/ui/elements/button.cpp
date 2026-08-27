/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/button.h"
#include "elements/elementdefinition.h"

namespace radia::ui {
namespace { constexpr char kElementName[] = "button"; }

ButtonElement::ButtonElement() : ButtonElement(kElementName) {}

ButtonElement::ButtonElement(const char* elementName) : Element(elementName) {}

ElementDefinition detail::ElementDefinitionFactory::button() {
    return defineElement<ButtonElement>(kElementName).build();
}
} // namespace radia::ui
