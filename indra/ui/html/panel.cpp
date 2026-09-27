/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/panel.h"
#include "html/elementnames.h"
#include "resource/elementdefinition.h"

namespace radia::ui {
HTMLPanelElement::HTMLPanelElement() : HTMLElement(HTMLTagName(HTMLTag::Panel)) {}

ResourceElementDefinition detail::ElementDefinitions::panel() {
    return defineElement<HTMLPanelElement>(HTMLTagName(HTMLTag::Panel)).attributes({allowedAttribute("filename")}).resourceRoot().build();
}
} // namespace radia::ui
