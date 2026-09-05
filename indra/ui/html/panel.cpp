/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/panel.h"
#include "html/elementnames.h"
#include "resource/elementdefinition.h"

namespace radia::ui {
HTMLPanelElement::HTMLPanelElement() : HTMLElement(kPanelTag.localName) {}

ResourceElementDefinition detail::ElementDefinitions::panel() {
    return defineElement<HTMLPanelElement>(kPanelTag.localName).attributes({allowedAttribute("filename")}).resourceRoot().build();
}
} // namespace radia::ui
