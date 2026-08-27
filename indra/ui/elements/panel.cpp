/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/panel.h"
#include "elements/elementdefinition.h"

namespace radia::ui {
namespace {
constexpr char kElementName[] = "panel";
}

PanelElement::PanelElement() : Element(kElementName) {}

ElementDefinition detail::ElementDefinitionFactory::panel() {
    return defineElement<PanelElement>(kElementName).attributes({allowedAttribute("filename")}).resourceRoot().build();
}
} // namespace radia::ui
