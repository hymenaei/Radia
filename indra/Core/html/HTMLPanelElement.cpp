/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "HTMLPanelElement.h"
#include "HTMLName.h"
#include "ResourceElementDefinition.h"

namespace Core {
HTMLPanelElement::HTMLPanelElement()
    : HTMLElement(HTMLTagName(HTMLTag::Panel)) {}

ResourceElementDefinition detail::ElementDefinitions::panel() {
    return defineElement<HTMLPanelElement>(HTMLTagName(HTMLTag::Panel)).attributes({allowedAttribute("filename")}).resourceRoot().build();
}
} // namespace Core
