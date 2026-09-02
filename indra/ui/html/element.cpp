/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/element.h"
#include "dom/elementinternal.h"
#include "html/elementfactory.h"
#include "html/elementnames.h"
#include "system.h"

namespace radia::ui {
using detail::appendText;
using detail::HTMLElementFactory;

HTMLElement::HTMLElement(std::string_view localName) : Element(localName) {}

void HTMLElement::setKeybinding(std::string keybindingId) {
    mKeybindingId = std::move(keybindingId);
    if (mKeybindingId.empty()) removeAttribute("shortcut");
    else setAttribute("shortcut", mKeybindingId);
}

std::string HTMLElement::textContent() const {
    if (elementName() == kBrTag.localName) return "\n";
    if (elementName() != kKbdTag.localName) return Element::textContent();

    std::string result;
    bool first = true;
    for (const Node* child : childNodes()) {
        if (const Element* element = child->asElement()) {
            if (!first) result += ' ';
            result += element->textContent();
        } else if (const Text* text = child->asText()) {
            result += text->data();
        }
        first = false;
    }
    return result;
}

void HTMLElement::onLocaleChanged(const System& system) {
    Element::onLocaleChanged(system);
    if (elementName() == kKbdTag.localName && !mKeybindingId.empty()) rebuildKeybindingContent(system);
}

void HTMLElement::onKeybindingsChanged(const System& system) {
    if (elementName() == kKbdTag.localName && !mKeybindingId.empty()) rebuildKeybindingContent(system);
}

void HTMLElement::rebuildKeybindingContent(const System& system) {
    replaceChildren();
    const KeybindingPresentation presentation = system.resolveKeybinding(mKeybindingId);
    for (const std::string& key : presentation.keys) {
        ElementPtr keyElement = HTMLElementFactory::Create(kKbdTag.localName);
        if (!keyElement) continue;
        appendText(*keyElement, key);
        append(std::move(keyElement));
    }
}
} // namespace radia::ui
