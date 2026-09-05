/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/elementfactory.h"
#include <memory>
#include "html/button.h"
#include "html/element.h"
#include "html/elementnames.h"
#include "html/fieldset.h"
#include "html/floater.h"
#include "html/icon.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"

namespace radia::ui::detail {
std::unique_ptr<Element> HTMLElementFactory::Create(std::string_view localName) {
    const HTMLTag tag = lookupHTMLTag(localName);
    switch (tag) {
        case HTMLTag::Button: return std::unique_ptr<HTMLButtonElement>(new HTMLButtonElement());
        case HTMLTag::Fieldset: return std::unique_ptr<HTMLFieldsetElement>(new HTMLFieldsetElement());
        case HTMLTag::Floater: return std::unique_ptr<HTMLFloaterElement>(new HTMLFloaterElement());
        case HTMLTag::Icon: return std::unique_ptr<HTMLIconElement>(new HTMLIconElement());
        case HTMLTag::Input: return std::unique_ptr<HTMLInputElement>(new HTMLInputElement());
        case HTMLTag::Label: return std::unique_ptr<HTMLLabelElement>(new HTMLLabelElement());
        case HTMLTag::Legend: return std::unique_ptr<HTMLLegendElement>(new HTMLLegendElement());
        case HTMLTag::Minimize: return std::unique_ptr<HTMLMinimizeButtonElement>(new HTMLMinimizeButtonElement());
        case HTMLTag::Close: return std::unique_ptr<HTMLCloseButtonElement>(new HTMLCloseButtonElement());
        case HTMLTag::Panel: return std::unique_ptr<HTMLPanelElement>(new HTMLPanelElement());
        case HTMLTag::Unknown: return nullptr;
        default: return std::unique_ptr<HTMLElement>(new HTMLElement(htmlTagName(tag)));
    }
}
} // namespace radia::ui::detail
