/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/elementdefinition.h"
#include "elements/input.h"
#include "style/style.h"

namespace radia::ui {
namespace {
class SwitchTrack final : public Element {
public:
    SwitchTrack() : Element("switch-track") { setPointerEvents(false); }
};

class SwitchThumb final : public Element {
public:
    SwitchThumb() : Element("switch-thumb") { setPointerEvents(false); }

protected:
    void constrainResolvedStyle(Style& style) const override {
        style.alignSelf = AlignSelf::Stretch;
        style.aspectRatio = 1.f;
    }
};
} // namespace

void InputElement::activateCheckbox() {
    activateChecked(!checked());
}

void InputElement::activateSwitch() {
    activateChecked(!checked());
}

InputElement& InputElement::indeterminate(bool indeterminate) {
    if (!isCheckboxType() || mIndeterminate == indeterminate) return *this;
    mIndeterminate = indeterminate;
    updateIndeterminateState(indeterminate);
    return *this;
}

void InputElement::resetIndeterminateState() {
    mIndeterminate = false;
    updateIndeterminateState(false);
}

void InputElement::refreshIndeterminateState() {
    updateIndeterminateState(isCheckboxType() && mIndeterminate);
}

bool InputElement::updateIndeterminateState(bool indeterminate) {
    const bool changed = hasState(ElementState::Indeterminate) != indeterminate;
    setState(ElementState::Indeterminate, indeterminate);
    return changed;
}

bool InputElement::shouldPaintChild(const Element& child, const Style& style) const {
    return !(isSwitchType() && style.appearance == AppearanceMode::Auto && (&child == mTrack || &child == mThumb));
}

void InputElement::onChildrenCleared() {
    mTrack = nullptr;
    mThumb = nullptr;
    if (isSwitchType()) detail::instantiateCompositeParts(*this, detail::ElementDefinitionFactory::input());
}

ElementDefinition detail::ElementDefinitionFactory::input() {
    return defineElement<InputElement>("input")
        .attributes({stringAttribute<InputElement>("type", &InputElement::type), stringAttribute<InputElement>("name", &InputElement::name),
                     booleanAttribute<InputElement>("switch", [](InputElement& element, bool enabled) { element.switchMode(enabled); }),
                     booleanAttribute<InputElement>("checked", [](InputElement& element, bool checked) { element.initializeChecked(checked); }),
                     stringAttribute("setting", &InputElement::setSettingName)})
        .validate([](const ElementBuildInput& input, InputElement& element, LayoutBuildResult& result, const LayoutBuildContext*) {
            const ElementAttribute* setting = input.find("setting");
            const ElementAttribute* checked = input.find("checked");
            const ElementAttribute* switchAttribute = input.find("switch");
            if (switchAttribute && schemaNameKey(element.type()) != "checkbox")
                result.error("layout.input.attribute_type", "The switch attribute requires a checkbox input type.", input.sourceName,
                             switchAttribute->source.begin.line, switchAttribute->source.begin.column);
            if ((setting || checked) && !InputElement::isCheckableType(element.type())) {
                const ElementAttribute* attribute = setting ? setting : checked;
                result.error("layout.input.attribute_type", "The checked and setting attributes require a checkable input type.", input.sourceName,
                             attribute->source.begin.line, attribute->source.begin.column);
            }
            if (setting && setting->value.empty())
                result.error("layout.value.setting_invalid", "Input setting must not be empty.", input.sourceName, setting->source.begin.line,
                             setting->source.begin.column);
            if (setting && checked)
                result.error("layout.value.multiple_sources", "An input cannot declare both setting and checked.", input.sourceName,
                             setting->source.begin.line, setting->source.begin.column);
        })
        .labelable()
        .state(ElementState::Checked)
        .state(ElementState::Indeterminate)
        .part<SwitchTrack>("track", &InputElement::mTrack)
        .part<SwitchThumb>("thumb", &InputElement::mThumb)
        .build();
}
} // namespace radia::ui
