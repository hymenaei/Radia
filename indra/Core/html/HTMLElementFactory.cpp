/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "HTMLElementFactory.h"
#include <memory>
#include "HTMLButtonElement.h"
#include "HTMLElement.h"
#include "HTMLFieldsetElement.h"
#include "HTMLFloaterElement.h"
#include "HTMLInputElement.h"
#include "HTMLLabelElement.h"
#include "HTMLName.h"
#include "HTMLPanelElement.h"

namespace Core::detail {
std::unique_ptr<Element> HTMLElementFactory::create(std::string_view localName) {
    const HTMLTag tag = findHTMLTag(localName);
    switch (tag) {
    case HTMLTag::Button:
        return std::unique_ptr<HTMLButtonElement>(new HTMLButtonElement());
    case HTMLTag::Fieldset:
        return std::unique_ptr<HTMLFieldsetElement>(new HTMLFieldsetElement());
    case HTMLTag::Floater:
        return std::unique_ptr<HTMLFloaterElement>(new HTMLFloaterElement());
    case HTMLTag::Input:
        return std::unique_ptr<HTMLInputElement>(new HTMLInputElement());
    case HTMLTag::Label:
        return std::unique_ptr<HTMLLabelElement>(new HTMLLabelElement());
    case HTMLTag::Legend:
        return std::unique_ptr<HTMLLegendElement>(new HTMLLegendElement());
    case HTMLTag::Minimize:
        return std::unique_ptr<HTMLMinimizeButtonElement>(new HTMLMinimizeButtonElement());
    case HTMLTag::Close:
        return std::unique_ptr<HTMLCloseButtonElement>(new HTMLCloseButtonElement());
    case HTMLTag::Panel:
        return std::unique_ptr<HTMLPanelElement>(new HTMLPanelElement());
    case HTMLTag::Unknown:
        return nullptr;
    default:
        return std::unique_ptr<HTMLElement>(new HTMLElement(HTMLTagName(tag)));
    }
}
} // namespace Core::detail
