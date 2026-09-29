/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "HTMLInputElement.h"
#include <algorithm>
#include <array>
#include <optional>
#include "Binder.h"
#include "ComputedStyle.h"
#include "ComputedStyleProperties.h"
#include "ElementInternal.h"
#include "HTMLLabelElement.h"
#include "HTMLName.h"
#include "InputTypes.h"
#include "LayoutGeometry.h"
#include "NativeAppearance.h"
#include "PaintContext.h"
#include "ResourceElementDefinition.h"
#include "Surface.h"

namespace Core {
using detail::ElementDefinitions;
using detail::ElementInternalAccess;

namespace {
class AttributeUpdateGuard final {
public:
    explicit AttributeUpdateGuard(bool& updating)
        : mUpdating(updating)
        , mPrevious(updating) {
        mUpdating = true;
    }
    ~AttributeUpdateGuard() { mUpdating = mPrevious; }

private:
    bool& mUpdating;
    bool mPrevious;
};

const Element* scopeRootForInput(const HTMLInputElement& input) {
    const Element* root = &input;
    while (root->parentElement() && !root->idScopeRoot())
        root = root->parentElement();
    return root;
}

void appendLabelName(const Element& root, const HTMLInputElement& input, std::string& name) {
    for (const Node* node : root.childNodes()) {
        const Element* child = node->asElement();
        if (!child || child->idScopeRoot())
            continue;
        if (const auto* label = dynamic_cast<const HTMLLabelElement*>(child); label && label->target() == &input) {
            const std::string labelText = label->textContent();
            if (!labelText.empty()) {
                if (!name.empty())
                    name += ' ';
                name += labelText;
            }
        }
        appendLabelName(*child, input, name);
    }
}

void paintInputOutline(PaintContext& context, const Layout::Rect& bounds, const Style::ComputedStyle& style) {
    if (style.outline().width <= 0.f || style.outline().color.resolvedColor().a <= 0.f)
        return;
    Style::ComputedStyle outlineStyle;
    outlineStyle.setBorderRadius(Style::BorderRadius {style.borderRadius()});
    outlineStyle.outline() = style.outline();
    context.paintBox(bounds, outlineStyle);
}
} // namespace

bool HTMLInputElement::isCheckableType(std::string_view type) {
    const std::optional<InputType> inputType = findInputType(canonicalizeHTMLName(type));
    return inputType == InputType::Checkbox || inputType == InputType::Radio;
}

bool HTMLInputElement::isCheckboxType() const { return findInputType(canonicalizeHTMLName(mType)) == InputType::Checkbox; }

bool HTMLInputElement::isRadioType() const { return findInputType(canonicalizeHTMLName(mType)) == InputType::Radio; }

bool HTMLInputElement::isSwitchType() const { return mSwitchMode && isCheckboxType(); }

HTMLInputElement::HTMLInputElement()
    : HTMLElement(HTMLTagName(HTMLTag::Input))
    , mSliderTrack(CSS::PseudoElement::SliderTrack, *this)
    , mSliderFill(CSS::PseudoElement::SliderFill, *this, &mSliderTrack)
    , mSliderThumb(CSS::PseudoElement::SliderThumb, *this)
    , mCheckmark(CSS::PseudoElement::Checkmark, *this) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    mSliderTrack.addGeneratedPseudoElement(mSliderFill);
    setAttribute("type", mType);
}

void HTMLInputElement::onAttributeSet(std::string_view name, const std::optional<std::string>& value) {
    if (mUpdatingAttribute)
        return;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (name == "type")
        type(value.value_or("text"));
    else if (name == "name")
        this->name(value.value_or(std::string()));
    else if (name == "switch")
        switchMode(true);
    else if (name == "checked" && !mValueState.dirty())
        initializeChecked(true);
    else if (name == "setting")
        setSettingName(value.value_or(std::string()));
}

void HTMLInputElement::onAttributeRemoved(std::string_view name) {
    if (mUpdatingAttribute)
        return;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (name == "type") {
        type("text");
        Element::removeAttribute("type");
    } else if (name == "name") {
        this->name({});
    } else if (name == "switch") {
        switchMode(false);
    } else if (name == "checked" && !mValueState.dirty()) {
        initializeChecked(false);
        Element::removeAttribute("checked");
    } else if (name == "setting") {
        clearValueBinding();
        mValueBindingRequest.reset();
    }
}

AccessibleSemantics HTMLInputElement::accessibleSemantics() const {
    AccessibleSemantics result = HTMLElement::accessibleSemantics();
    if (isSwitchType())
        result.role = AccessibleRole::Switch;
    else if (isRadioType())
        result.role = AccessibleRole::Radio;
    else if (isCheckboxType())
        result.role = AccessibleRole::Checkbox;
    else
        result.role = AccessibleRole::TextInput;
    result.name.clear();
    appendLabelName(*scopeRootForInput(*this), *this, result.name);
    if (isCheckableType(mType)) {
        result.checked = checked();
        result.indeterminate = indeterminate();
    }
    return result;
}

void HTMLInputElement::constrainResolvedStyle(Style::ComputedStyle& style) const {
    if (style.appearance() != Style::Appearance::Base || !isCheckableType(mType) || isSwitchType())
        return;

    const auto wasSpecified = [&style](CSS::Property property) {
        const std::string_view name = CSS::propertyName(property);
        return std::find(style.specifiedProperties.begin(), style.specifiedProperties.end(), name) != style.specifiedProperties.end();
    };

    Layout::RectEdges<Style::LineWidth> widths = style.borderWidth();
    if (!wasSpecified(CSS::Property::BorderTopWidth))
        widths.top = Style::LineWidth {1.f};
    if (!wasSpecified(CSS::Property::BorderRightWidth))
        widths.right = Style::LineWidth {1.f};
    if (!wasSpecified(CSS::Property::BorderBottomWidth))
        widths.bottom = Style::LineWidth {1.f};
    if (!wasSpecified(CSS::Property::BorderLeftWidth))
        widths.left = Style::LineWidth {1.f};
    style.setBorderWidth(std::move(widths));

    Layout::RectEdges<Style::BorderStyle> styles = style.borderStyle();
    if (!wasSpecified(CSS::Property::BorderTopStyle))
        styles.top = Style::BorderStyle::Solid;
    if (!wasSpecified(CSS::Property::BorderRightStyle))
        styles.right = Style::BorderStyle::Solid;
    if (!wasSpecified(CSS::Property::BorderBottomStyle))
        styles.bottom = Style::BorderStyle::Solid;
    if (!wasSpecified(CSS::Property::BorderLeftStyle))
        styles.left = Style::BorderStyle::Solid;
    style.setBorderStyle(std::move(styles));
}

Layout::Vec2 HTMLInputElement::intrinsicSize(const CSS::StyleSheet&, const Style::ComputedStyle& style, const TextMeasurer&,
    const Layout::IntrinsicSizeConstraints& constraints) const {
    if (!isCheckableType(mType))
        return {};
    if (style.appearance() != Style::Appearance::Auto && !isSwitchType()) {
        const Layout::RectEdges<float> borderInsets = Layout::borderWidths(style);
        const float size = std::max(24.f, style.fontSize());
        return {std::max(0.f, size - Layout::paddingPixels(style).horizontal() - borderInsets.horizontal()),
            std::max(0.f, size - Layout::paddingPixels(style).vertical() - borderInsets.vertical())};
    }
    if (style.appearance() != Style::Appearance::Auto)
        return {};
    NativeInputControl control = NativeInputControl::Checkbox;
    if (isRadioType())
        control = NativeInputControl::Radio;
    else if (isSwitchType())
        control = NativeInputControl::Switch;
    const NativeLayoutMetrics metrics =
        constraints.nativeMetrics.value_or(surface() ? surface()->nativeLayoutMetrics() : defaultNativeLayoutMetrics());
    return metrics.inputMetrics(control).intrinsicSize;
}

void HTMLInputElement::paint(PaintContext& context, const Style::ComputedStyle& style, float scale) const {
    if (style.appearance() != Style::Appearance::Auto || !isCheckableType(mType)) {
        Element::paint(context, style, scale);
        if (style.appearance() != Style::Appearance::Auto) {
            const bool clipsX = style.overflowX() != Style::Overflow::Visible;
            const bool clipsY = style.overflowY() != Style::Overflow::Visible;
            const Layout::ClipAxes clipAxes =
                (clipsX ? Layout::ClipAxes::X : Layout::ClipAxes::NoAxes) | (clipsY ? Layout::ClipAxes::Y : Layout::ClipAxes::NoAxes);
            if (clipsX || clipsY)
                context.pushClip(ElementInternalAccess::scrollport(*this), scale, clipAxes);
            const auto paintPseudoElement = [&context, scale](const Style::PseudoElement& pseudoElement, float inheritedOpacity,
                                                const auto& paintChildren) -> void {
                const Style::ComputedStyle& pseudoStyle = pseudoElement.style();
                if (pseudoStyle.display() == Style::Display::NoneValue || pseudoElement.rect().empty())
                    return;
                Style::ComputedStyle paintedStyle = pseudoStyle;
                applyOpacity(paintedStyle, inheritedOpacity);
                const bool clipsX = paintedStyle.overflowX() != Style::Overflow::Visible;
                const bool clipsY = paintedStyle.overflowY() != Style::Overflow::Visible;
                const Layout::ClipAxes clipAxes =
                    (clipsX ? Layout::ClipAxes::X : Layout::ClipAxes::NoAxes) | (clipsY ? Layout::ClipAxes::Y : Layout::ClipAxes::NoAxes);
                if (clipsX || clipsY)
                    context.pushClip(pseudoElement.rect(), scale, clipAxes);
                if (paintedStyle.visibility() == Style::Visibility::Visible) {
                    context.paintBox(pseudoElement.rect(), paintedStyle);
                    if (paintedStyle.content && !paintedStyle.content->empty()) {
                        const Layout::RectEdges<float> borderInsets = Layout::borderWidths(paintedStyle);
                        const Layout::RectEdges<float> contentInsets {
                            Layout::paddingPixels(paintedStyle).top + borderInsets.top,
                            Layout::paddingPixels(paintedStyle).right + borderInsets.right,
                            Layout::paddingPixels(paintedStyle).bottom + borderInsets.bottom,
                            Layout::paddingPixels(paintedStyle).left + borderInsets.left,
                        };
                        context.paintText(*paintedStyle.content, Layout::insetRect(pseudoElement.rect(), contentInsets), paintedStyle);
                    }
                }
                for (const Style::PseudoElement* child : pseudoElement.generatedPseudoElements())
                    if (child)
                        paintChildren(*child, paintedStyle.opacity().value, paintChildren);
                if (clipsX || clipsY)
                    context.popClip();
            };
            if (isSwitchType()) {
                if (const Style::PseudoElement* sliderTrack = this->sliderTrack(); sliderTrack)
                    paintPseudoElement(*sliderTrack, style.opacity().value, paintPseudoElement);
                if (const Style::PseudoElement* sliderThumb = this->sliderThumb(); sliderThumb)
                    paintPseudoElement(*sliderThumb, style.opacity().value, paintPseudoElement);
            } else if (const Style::PseudoElement* checkmark = this->checkmark()) {
                paintPseudoElement(*checkmark, style.opacity().value, paintPseudoElement);
            }
            if (clipsX || clipsY)
                context.popClip();
        }
        return;
    }

    NativeInputPaintRequest request;
    if (isRadioType())
        request.control = NativeInputControl::Radio;
    else if (isSwitchType())
        request.control = NativeInputControl::Switch;
    else
        request.control = NativeInputControl::Checkbox;
    request.bounds = rect();
    request.checked = checked();
    request.indeterminate = indeterminate();
    request.disabled = disabled();
    request.hovered = hovered();
    request.pressed = active();
    request.opacity = style.opacity().value;
    const Style::AccentColor& accent = style.accentColor();
    if (!accent.isKeyword()) {
        const Style::Color& value = accent.value();
        request.accentColor = value.resolvedColor();
    }
    request.colorScheme = style.usedColorScheme;
    request.direction = style.direction;
    request.scale = scale;
    context.paintNativeInput(request);
    paintInputOutline(context, rect(), style);
}

HTMLInputElement& HTMLInputElement::type(std::string type) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (type.empty())
        type = "text";
    if (canonicalizeHTMLName(mType) == canonicalizeHTMLName(type)) {
        mType = std::move(type);
        setAttribute("type", mType);
        return *this;
    }

    const bool wasRadio = isRadioType();
    const std::string oldName = mName;
    if (wasRadio)
        refreshRadioGroup(oldName, this);

    clearValueBinding();
    mValueBindingRequest.reset();
    const bool wasInvalid = invalid();
    updateCheckedState(false);
    updateIndeterminateState(false);
    mValueState = {};
    if (wasInvalid)
        invalidatePseudoClass(CSS::PseudoClass::Invalid);
    mSwitchMode = false;
    removeAttribute("checked");
    removeAttribute("switch");
    removeAttribute("setting");

    mType = std::move(type);
    setAttribute("type", mType);

    if (isRadioType())
        refreshRadioGroup();
    else
        refreshIndeterminateState();
    return *this;
}

HTMLInputElement& HTMLInputElement::name(std::string name) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (mName == name)
        return *this;
    const std::string oldName = mName;
    if (isRadioType())
        refreshRadioGroup(oldName, this);
    mName = std::move(name);
    if (mName.empty())
        removeAttribute("name");
    else
        setAttribute("name", mName);
    if (isRadioType())
        refreshRadioGroup();
    return *this;
}

HTMLInputElement& HTMLInputElement::switchMode(bool enabled) {
    const bool updateAttribute = !mUpdatingAttribute;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (mSwitchMode == enabled)
        return *this;
    if (enabled && !isCheckboxType())
        return *this;

    mSwitchMode = enabled;
    if (updateAttribute) {
        if (enabled)
            setAttribute("switch");
        else
            removeAttribute("switch");
    }

    return *this;
}

HTMLInputElement& HTMLInputElement::checked(bool checked) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (!isCheckableType(mType))
        return *this;
    const ElementRef<HTMLInputElement> self(this);
    const bool changed = updateCheckedState(checked);
    if (checked)
        setAttribute("checked");
    else
        removeAttribute("checked");
    if (isRadioType())
        updateRadioGroup();
    else
        refreshIndeterminateState();
    if (!self)
        return *this;
    if (changed)
        notifyValueState();
    return *this;
}

void HTMLInputElement::initializeChecked(bool checked) {
    const bool updateAttribute = !mUpdatingAttribute;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (!isCheckableType(mType))
        return;
    const bool wasInvalid = invalid();
    updateCheckedState(checked);
    mValueState.baseline = checked;
    mValueState.validation.reset();
    if (wasInvalid)
        invalidatePseudoClass(CSS::PseudoClass::Invalid);
    if (updateAttribute) {
        if (checked)
            setAttribute("checked");
        else
            removeAttribute("checked");
    }
    if (isRadioType())
        updateRadioGroup();
    else
        refreshIndeterminateState();
}

bool HTMLInputElement::updateCheckedState(bool checked) {
    if (!isCheckableType(mType))
        return false;
    const bool changed = setPseudoClassMatch(CSS::PseudoClass::Checked, mValueState.value, checked);
    if (changed)
        invalidateArrange();
    return changed;
}

void HTMLInputElement::clearValueBinding() {
    if (const std::shared_ptr<ValueBindingSubscription> subscription = mBindingSubscription.lock())
        subscription->reset();
    mBindingSubscription.reset();
    mBinding.reset();
}

HTMLInputElement& HTMLInputElement::setSettingName(std::string name) {
    if (mValueBindingRequest && mValueBindingRequest->settingName == name)
        return *this;
    clearValueBinding();
    mValueBindingRequest = ValueBindingRequest {std::move(name)};
    return *this;
}

InputValueState HTMLInputElement::valueState() const {
    InputValueState result;
    result.dirty = mValueState.dirty();
    result.validationStatus = mValueState.validationStatus();
    if (const std::string* message = mValueState.validationMessage())
        result.validationMessage = *message;
    return result;
}

ValueBindingSubscription HTMLInputElement::observeValueState(ValueStateObserver observer) {
    const std::size_t id = mNextValueObserver++;
    mValueObservers.emplace(id, std::move(observer));
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    return ValueBindingSubscription([this, lifetime, id] {
        if (!lifetime.expired())
            mValueObservers.erase(id);
    });
}

void HTMLInputElement::notifyValueState() {
    const InputValueState state = valueState();
    const auto observers = mValueObservers;
    const std::weak_ptr<char> lifetime = mValueObserverLifetime;
    for (const auto& [id, observer] : observers) {
        if (lifetime.expired())
            return;
        if (mValueObservers.find(id) != mValueObservers.end())
            observer(state);
    }
}

void HTMLInputElement::applyValueState(ValueState<bool> state) {
    const ElementRef<HTMLInputElement> self(this);
    const bool changed = mValueState != state;
    const bool checkedChanged = isCheckableType(mType) && mValueState.value != state.value;
    const bool invalidChanged = invalid() != (state.validationStatus() == ValueValidationStatus::Invalid);
    mValueState = std::move(state);
    if (checkedChanged) {
        invalidatePseudoClass(CSS::PseudoClass::Checked);
        invalidateArrange();
    }
    if (invalidChanged)
        invalidatePseudoClass(CSS::PseudoClass::Invalid);
    if (isRadioType())
        updateRadioGroup();
    else
        refreshIndeterminateState();
    if (!self)
        return;
    if (changed)
        notifyValueState();
}

void HTMLInputElement::synchronizeValueBinding() {
    if (mBinding)
        applyValueState(mBinding->state());
}

void HTMLInputElement::prepareValueBinding(Binder& binder) {
    if (mValueBindingRequest)
        binder.requireValueBinding(*mValueBindingRequest, mBinding);
}

ValueBindingSubscription HTMLInputElement::commitValueBinding(const std::shared_ptr<bool>& bindingActive, Element& root) {
    if (!mBinding)
        return {};
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    const std::weak_ptr<bool> active = bindingActive;
    const ElementRef<Element> rootRef(&root);
    const ElementRef<HTMLInputElement> inputRef(this);
    std::shared_ptr<ValueBinding<bool>> provider = mBinding.shared();
    const ValueBinding<bool>* providerPointer = provider.get();
    const auto isValid = [&] {
        HTMLInputElement* input = inputRef.get();
        Element* rootElement = rootRef.get();
        if (!input || !rootElement || input->mBinding.shared() != provider)
            return false;
        for (Node* current = input; current; current = current->parentNode())
            if (current == rootElement)
                return true;
        return false;
    };
    applyValueState(provider->state());
    if (!isValid())
        return {};
    auto providerSubscription = std::make_shared<ValueBindingSubscription>(
        provider->observe([lifetime, active, rootRef, inputRef, providerPointer](const ValueState<bool>& state) {
            const std::shared_ptr<bool> bindingIsActive = active.lock();
            Element* rootElement = rootRef.get();
            HTMLInputElement* input = inputRef.get();
            if (!lifetime.expired() && bindingIsActive && *bindingIsActive && rootElement && input
                && input->mBinding.shared().get() == providerPointer) {
                for (Element* current = input; current; current = current->parentElement()) {
                    if (current == rootElement) {
                        input->applyValueState(state);
                        break;
                    }
                }
            }
        }));
    if (!isValid()) {
        providerSubscription->reset();
        return {};
    }
    return ValueBindingSubscription([this, lifetime, provider = std::move(provider), providerSubscription] {
        providerSubscription->reset();
        if (!lifetime.expired() && mBinding.shared() == provider)
            mBinding.reset();
    });
}

HTMLInputElement& HTMLInputElement::setOnCheckedChanged(std::function<void(bool)> callback) {
    mOnCheckedChanged = std::move(callback);
    return *this;
}

void HTMLInputElement::activateChecked(bool checked) {
    ElementRef<HTMLInputElement> self(this);
    const bool previous = this->checked();
    if (mBinding) {
        const std::shared_ptr<ValueBinding<bool>> provider = mBinding.shared();
        provider->write(checked);
        HTMLInputElement* current = self.get();
        if (!current || current->mBinding.shared() != provider)
            return;
        current->applyValueState(provider->state());
    } else {
        updateCheckedState(checked);
        if (isRadioType())
            updateRadioGroup();
        else
            refreshIndeterminateState();
        if (!self)
            return;
        notifyValueState();
    }
    HTMLInputElement* current = self.get();
    if (!current || current->checked() == previous)
        return;
    if (current->mOnCheckedChanged)
        current->mOnCheckedChanged(current->checked());
    current = self.get();
    if (!current)
        return;
    Event input(kInputEvent, *current, current->checked());
    current->dispatchEvent(input);
    current = self.get();
    if (!current)
        return;
    Event change(kChangeEvent, *current, current->checked());
    current->dispatchEvent(change);
}

void HTMLInputElement::onTreeAttached() {
    if (isRadioType())
        updateRadioGroup();
    else
        refreshIndeterminateState();
}

void HTMLInputElement::onTreeWillBeDetached() {
    if (isRadioType())
        refreshRadioGroup(mName, this);
}

void HTMLInputElement::onActivate() {
    const std::string key = canonicalizeHTMLName(mType);
    if (key == "checkbox") {
        indeterminate(false);
        if (isSwitchType())
            activateSwitch();
        else
            activateCheckbox();
    } else if (key == "radio")
        activateRadio();
}

std::vector<Style::PseudoElement*> HTMLInputElement::generatedPseudoElements() const {
    if (isSwitchType())
        return {&mSliderTrack, &mSliderThumb};
    if (isCheckableType(mType))
        return {&mCheckmark};
    return {};
}
} // namespace Core
