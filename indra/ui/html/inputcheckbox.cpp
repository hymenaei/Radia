/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/elementinternal.h"
#include "html/elementnames.h"
#include "html/input.h"
#include "resource/elementdefinition.h"
#include "style/style.h"

namespace radia::ui {
using detail::ElementDefinitions;

void HTMLInputElement::activateCheckbox() {
    activateChecked(!checked());
}

void HTMLInputElement::activateSwitch() {
    activateChecked(!checked());
}

HTMLInputElement& HTMLInputElement::indeterminate(bool indeterminate) {
    if (!isCheckboxType() || mIndeterminate == indeterminate) return *this;
    mIndeterminate = indeterminate;
    updateIndeterminateState(indeterminate);
    return *this;
}

void HTMLInputElement::resetIndeterminateState() {
    mIndeterminate = false;
    updateIndeterminateState(false);
}

void HTMLInputElement::refreshIndeterminateState() {
    updateIndeterminateState(isCheckboxType() && mIndeterminate);
}

bool HTMLInputElement::updateIndeterminateState(bool indeterminate) {
    const bool changed = hasState(ElementState::Indeterminate) != indeterminate;
    setState(ElementState::Indeterminate, indeterminate);
    return changed;
}

ResourceElementDefinition detail::ElementDefinitions::input() {
    return defineElement<HTMLInputElement>(kInputTag.localName)
        .attributes(
            {stringAttribute<HTMLInputElement>("type", &HTMLInputElement::type), stringAttribute<HTMLInputElement>("name", &HTMLInputElement::name),
             booleanAttribute<HTMLInputElement>("switch", [](HTMLInputElement& element, bool enabled) { element.switchMode(enabled); }),
             booleanAttribute<HTMLInputElement>("checked", [](HTMLInputElement& element, bool checked) { element.initializeChecked(checked); }),
             stringAttribute("setting", &HTMLInputElement::setSettingName)})
        .validate([](const ElementBuildInput& input, HTMLInputElement& element, ElementBuildContext& context) {
            const ElementAttribute* setting = input.find("setting");
            const ElementAttribute* checked = input.find("checked");
            const ElementAttribute* switchAttribute = input.find("switch");
            if (switchAttribute && canonicalizeHTMLName(element.type()) != "checkbox")
                context.error("layout.input.attribute_type", "The switch attribute requires a checkbox input type.", input.sourceName,
                              switchAttribute->source.begin.line, switchAttribute->source.begin.column);
            if ((setting || checked) && !HTMLInputElement::isCheckableType(element.type())) {
                const ElementAttribute* attribute = setting ? setting : checked;
                context.error("layout.input.attribute_type", "The checked and setting attributes require a checkable input type.", input.sourceName,
                              attribute->source.begin.line, attribute->source.begin.column);
            }
            if (setting && setting->value.empty())
                context.error("layout.value.setting_invalid", "Input setting must not be empty.", input.sourceName, setting->source.begin.line,
                              setting->source.begin.column);
            if (setting && checked)
                context.error("layout.value.multiple_sources", "An input cannot declare both setting and checked.", input.sourceName,
                              setting->source.begin.line, setting->source.begin.column);
        })
        .labelable()
        .state(ElementState::Checked)
        .state(ElementState::Indeterminate)
        .pseudoElement("slider-track")
        .pseudoElement("slider-fill")
        .pseudoElement("slider-thumb")
        .pseudoElement("checkmark")
        .build();
}
} // namespace radia::ui
