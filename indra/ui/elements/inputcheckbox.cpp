/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include "elements/elementdefinition.h"
#include "elements/input.h"
#include "render/paintcontext.h"
#include "style/style.h"

namespace radia::ui {
namespace {
constexpr float kCheckboxSize = 13.f;
constexpr float kSwitchWidth = 36.f;
constexpr float kSwitchHeight = 20.f;

Color switchTrack(bool checked, bool disabled) {
    const Color color = checked ? Color(.12f, .42f, .86f) : Color(.45f, .45f, .48f);
    return disabled ? color.withAlpha(.55f) : color;
}

Color switchThumb(bool disabled) {
    const Color color(.98f, .98f, .98f);
    return disabled ? color.withAlpha(.65f) : color;
}

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

Vec2 InputElement::nativeCheckboxIntrinsicSize() const {
    return {kCheckboxSize, kCheckboxSize};
}

Vec2 InputElement::nativeSwitchIntrinsicSize() const {
    return {kSwitchWidth, kSwitchHeight};
}

void InputElement::paintNativeSwitch(PaintContext& context, const Style& style, float) const {
    const Rect bounds = rect();
    const float radius = std::max(0.f, bounds.h * .5f);
    context.paintBox(bounds, nativeControlStyle(style, switchTrack(checked(), disabled()), nativeControlBorder(disabled()), radius));

    const float inset = std::min(2.f, std::max(0.f, std::min(bounds.w, bounds.h) * .5f));
    const float thumbSize = std::max(0.f, bounds.h - inset * 2.f);
    const bool thumbAtRight = style.direction == LayoutDirection::LeftToRight ? checked() : !checked();
    const float thumbLeft = thumbAtRight ? bounds.right() - inset - thumbSize : bounds.left() + inset;
    const Rect thumb{thumbLeft, bounds.bottom() + inset, thumbSize, thumbSize};
    context.paintBox(thumb, nativeMarkStyle(style, switchThumb(disabled()), thumbSize * .5f));
}

void InputElement::paintNativeCheckbox(PaintContext& context, const Style& style, float) const {
    const bool selected = checked() || indeterminate();
    const Rect bounds = rect();
    context.paintBox(bounds, nativeControlStyle(style, nativeControlFill(selected, disabled()), nativeControlBorder(disabled()), 2.f));
    if (!selected) return;

    Style mark = nativeMarkStyle(style, nativeControlMark(disabled()));
    mark.backgroundColor = {};
    mark.textColor = nativeControlMark(disabled());
    mark.textAlign = TextAlign::Center;
    mark.fontSize = 11.f;
    context.paintText(indeterminate() ? "−" : "✓", bounds, mark);
}

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
