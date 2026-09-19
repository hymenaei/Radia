/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "html/input.h"
#include <algorithm>
#include "binding/binder.h"
#include "dom/elementinternal.h"
#include "html/elementnames.h"
#include "html/label.h"
#include "paint/nativeappearance.h"
#include "paint/paintcontext.h"
#include "resource/elementdefinition.h"
#include "style/computedstyle.h"
#include "surface/surface.h"

namespace radia::ui {
using detail::ElementDefinitions;
using detail::ElementInternalAccess;

namespace {
class AttributeUpdateGuard final {
public:
    explicit AttributeUpdateGuard(bool& updating) : mUpdating(updating), mPrevious(updating) { mUpdating = true; }
    ~AttributeUpdateGuard() { mUpdating = mPrevious; }

private:
    bool& mUpdating;
    bool mPrevious;
};

const Element* scopeRootForInput(const HTMLInputElement& input) {
    const Element* root = &input;
    while (root->parentElement() && !root->idScopeRoot()) root = root->parentElement();
    return root;
}

void appendLabelName(const Element& root, const HTMLInputElement& input, std::string& name) {
    for (const Node* node : root.childNodes()) {
        const Element* child = node->asElement();
        if (!child || child->idScopeRoot()) continue;
        if (const auto* label = dynamic_cast<const HTMLLabelElement*>(child); label && label->target() == &input) {
            const std::string labelText = label->textContent();
            if (!labelText.empty()) {
                if (!name.empty()) name += ' ';
                name += labelText;
            }
        }
        appendLabelName(*child, input, name);
    }
}

void paintInputOutline(PaintContext& context, const Rect& bounds, const ComputedStyle& style) {
    if (style.outline.width <= 0.f || style.outline.color.a <= 0.f) return;
    ComputedStyle outlineStyle;
    outlineStyle.borderRadius = style.borderRadius;
    outlineStyle.outline = style.outline;
    context.paintBox(bounds, outlineStyle);
}
} // namespace

bool HTMLInputElement::isCheckableType(std::string_view type) {
    const std::string key = canonicalizeHTMLName(type);
    return key == "checkbox" || key == "radio";
}

bool HTMLInputElement::isCheckboxType() const {
    return canonicalizeHTMLName(mType) == "checkbox";
}

bool HTMLInputElement::isRadioType() const {
    return canonicalizeHTMLName(mType) == "radio";
}

bool HTMLInputElement::isSwitchType() const {
    return mSwitchMode && isCheckboxType();
}

HTMLInputElement::HTMLInputElement()
    : HTMLElement(kInputTag.localName), mSliderTrack(PseudoElementType::SliderTrack, *this),
      mSliderFill(PseudoElementType::SliderFill, *this, &mSliderTrack), mSliderThumb(PseudoElementType::SliderThumb, *this),
      mCheckmark(PseudoElementType::Checkmark, *this) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    mSliderTrack.addGeneratedPseudoElement(mSliderFill);
    setAttribute("type", mType);
}

void HTMLInputElement::onAttributeSet(std::string_view name, const std::optional<std::string>& value) {
    if (mUpdatingAttribute) return;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (name == "type") type(value.value_or("text"));
    else if (name == "name") this->name(value.value_or(std::string()));
    else if (name == "switch") switchMode(true);
    else if (name == "checked" && !mValueState.dirty()) initializeChecked(true);
    else if (name == "setting") setSettingName(value.value_or(std::string()));
}

void HTMLInputElement::onAttributeRemoved(std::string_view name) {
    if (mUpdatingAttribute) return;
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
    if (isSwitchType()) result.role = AccessibleRole::Switch;
    else if (isRadioType()) result.role = AccessibleRole::Radio;
    else if (isCheckboxType()) result.role = AccessibleRole::Checkbox;
    else result.role = AccessibleRole::TextInput;
    result.name.clear();
    appendLabelName(*scopeRootForInput(*this), *this, result.name);
    if (isCheckableType(mType)) {
        result.checked = checked();
        result.indeterminate = indeterminate();
    }
    return result;
}

void HTMLInputElement::constrainResolvedStyle(ComputedStyle& style) const {
    if (style.appearance != AppearanceMode::Base || !isCheckableType(mType) || isSwitchType() || style.borderWidthSet) return;
    style.borderWidth = {1.f, 1.f, 1.f, 1.f};
    style.borderStyle = BorderStyle::Solid;
    if (!style.borderColorSet) {
        style.borderColor = {};
        style.borderColorLightDark.reset();
        style.borderGradient.reset();
        style.borderColorCurrent = true;
    }
}

Vec2 HTMLInputElement::intrinsicSize(const StyleSheet&, const ComputedStyle& style, const TextMetrics&,
                                     const IntrinsicSizeConstraints& constraints) const {
    if (!isCheckableType(mType)) return {};
    if (style.appearance != AppearanceMode::Auto && !isSwitchType()) {
        const float size = std::max(24.f, style.fontSize);
        return {std::max(0.f, size - style.padding.horizontal() - style.borderWidth.horizontal()),
                std::max(0.f, size - style.padding.vertical() - style.borderWidth.vertical())};
    }
    if (style.appearance != AppearanceMode::Auto) return {};
    NativeInputControl control = NativeInputControl::Checkbox;
    if (isRadioType()) control = NativeInputControl::Radio;
    else if (isSwitchType()) control = NativeInputControl::Switch;
    const NativeLayoutMetrics metrics =
        constraints.nativeMetrics.value_or(surface() ? surface()->nativeLayoutMetrics() : defaultNativeLayoutMetrics());
    return metrics.inputMetrics(control).intrinsicSize;
}

void HTMLInputElement::paint(PaintContext& context, const ComputedStyle& style, float scale) const {
    if (style.appearance != AppearanceMode::Auto || !isCheckableType(mType)) {
        Element::paint(context, style, scale);
        if (style.appearance != AppearanceMode::Auto) {
            const bool clipsX = style.overflowX != Overflow::Visible;
            const bool clipsY = style.overflowY != Overflow::Visible;
            const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
            if (clipsX || clipsY) context.pushClip(ElementInternalAccess::scrollport(*this), scale, clipAxes);
            const auto paintPseudoElement = [&context, scale](const PseudoElement& pseudoElement, float inheritedOpacity,
                                                              const auto& paintChildren) -> void {
                const ComputedStyle& pseudoStyle = pseudoElement.style();
                if (pseudoStyle.display == DisplayMode::NoneValue || pseudoElement.rect().empty()) return;
                ComputedStyle paintedStyle = pseudoStyle;
                applyOpacity(paintedStyle, inheritedOpacity);
                const bool clipsX = paintedStyle.overflowX != Overflow::Visible;
                const bool clipsY = paintedStyle.overflowY != Overflow::Visible;
                const ClipAxes clipAxes = (clipsX ? ClipAxes::X : ClipAxes::NoAxes) | (clipsY ? ClipAxes::Y : ClipAxes::NoAxes);
                if (clipsX || clipsY) context.pushClip(pseudoElement.rect(), scale, clipAxes);
                if (paintedStyle.visibility == Visibility::Visible) {
                    context.paintBox(pseudoElement.rect(), paintedStyle);
                    if (paintedStyle.content && !paintedStyle.content->empty()) {
                        const EdgeInsets contentInsets{
                            paintedStyle.padding.top + paintedStyle.borderWidth.top,
                            paintedStyle.padding.right + paintedStyle.borderWidth.right,
                            paintedStyle.padding.bottom + paintedStyle.borderWidth.bottom,
                            paintedStyle.padding.left + paintedStyle.borderWidth.left,
                        };
                        context.paintText(*paintedStyle.content, insetRect(pseudoElement.rect(), contentInsets), paintedStyle);
                    }
                }
                for (const PseudoElement* child : pseudoElement.generatedPseudoElements())
                    if (child) paintChildren(*child, paintedStyle.opacity, paintChildren);
                if (clipsX || clipsY) context.popClip();
            };
            if (isSwitchType()) {
                if (const PseudoElement* sliderTrack = this->sliderTrack(); sliderTrack)
                    paintPseudoElement(*sliderTrack, style.opacity, paintPseudoElement);
                if (const PseudoElement* sliderThumb = this->sliderThumb(); sliderThumb)
                    paintPseudoElement(*sliderThumb, style.opacity, paintPseudoElement);
            } else if (const PseudoElement* checkmark = this->checkmark()) {
                paintPseudoElement(*checkmark, style.opacity, paintPseudoElement);
            }
            if (clipsX || clipsY) context.popClip();
        }
        return;
    }

    NativeInputPaintRequest request;
    if (isRadioType()) request.control = NativeInputControl::Radio;
    else if (isSwitchType()) request.control = NativeInputControl::Switch;
    else request.control = NativeInputControl::Checkbox;
    request.bounds = rect();
    request.checked = checked();
    request.indeterminate = indeterminate();
    request.disabled = disabled();
    request.hovered = hasState(ElementState::Hovered);
    request.pressed = hasState(ElementState::Active);
    request.opacity = style.opacity;
    if (style.accentColor.kind == AccentColor::Kind::CurrentColor) request.accentColor = style.color;
    else if (style.accentColor.kind == AccentColor::Kind::Color) request.accentColor = style.accentColor.color;
    request.colorScheme = style.usedColorScheme;
    request.direction = style.direction;
    request.scale = scale;
    context.paintNativeInput(request);
    paintInputOutline(context, rect(), style);
}

HTMLInputElement& HTMLInputElement::type(std::string type) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (type.empty()) type = "text";
    if (canonicalizeHTMLName(mType) == canonicalizeHTMLName(type)) {
        mType = std::move(type);
        setAttribute("type", mType);
        return *this;
    }

    const bool wasRadio = isRadioType();
    const std::string oldName = mName;
    if (wasRadio) refreshRadioGroup(oldName, this);

    clearValueBinding();
    mValueBindingRequest.reset();
    mValueState = {};
    updateCheckedState(false);
    mIndeterminate = false;
    updateIndeterminateState(false);
    mSwitchMode = false;
    removeAttribute("checked");
    removeAttribute("switch");
    removeAttribute("setting");

    mType = std::move(type);
    setAttribute("type", mType);

    if (isRadioType()) refreshRadioGroup();
    else refreshIndeterminateState();
    return *this;
}

HTMLInputElement& HTMLInputElement::name(std::string name) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (mName == name) return *this;
    const std::string oldName = mName;
    if (isRadioType()) refreshRadioGroup(oldName, this);
    mName = std::move(name);
    if (mName.empty()) removeAttribute("name");
    else setAttribute("name", mName);
    if (isRadioType()) refreshRadioGroup();
    return *this;
}

HTMLInputElement& HTMLInputElement::switchMode(bool enabled) {
    const bool updateAttribute = !mUpdatingAttribute;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (mSwitchMode == enabled) return *this;
    if (enabled && !isCheckboxType()) return *this;

    mSwitchMode = enabled;
    if (updateAttribute) {
        if (enabled) setAttribute("switch");
        else removeAttribute("switch");
    }

    return *this;
}

HTMLInputElement& HTMLInputElement::checked(bool checked) {
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (!isCheckableType(mType)) return *this;
    const ElementRef<HTMLInputElement> self(this);
    const bool changed = updateCheckedState(checked);
    mValueState.value = checked;
    if (checked) setAttribute("checked");
    else removeAttribute("checked");
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
    if (!self) return *this;
    if (changed) notifyValueState();
    return *this;
}

void HTMLInputElement::initializeChecked(bool checked) {
    const bool updateAttribute = !mUpdatingAttribute;
    AttributeUpdateGuard guard(mUpdatingAttribute);
    if (!isCheckableType(mType)) return;
    mValueState = {checked, checked, std::nullopt};
    updateCheckedState(checked);
    if (updateAttribute) {
        if (checked) setAttribute("checked");
        else removeAttribute("checked");
    }
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
}

bool HTMLInputElement::updateCheckedState(bool checked) {
    const bool changed = checked != this->checked();
    setState(ElementState::Checked, checked);
    if (changed) invalidateArrange();
    return changed;
}

void HTMLInputElement::clearValueBinding() {
    if (const std::shared_ptr<ValueBindingSubscription> subscription = mBindingSubscription.lock()) subscription->reset();
    mBindingSubscription.reset();
    mBinding.reset();
}

HTMLInputElement& HTMLInputElement::setSettingName(std::string name) {
    if (mValueBindingRequest && mValueBindingRequest->settingName == name) return *this;
    clearValueBinding();
    mValueBindingRequest = ValueBindingRequest{std::move(name)};
    return *this;
}

InputValueState HTMLInputElement::valueState() const {
    InputValueState result;
    result.dirty = mValueState.dirty();
    result.validationStatus = mValueState.validationStatus();
    if (const std::string* message = mValueState.validationMessage()) result.validationMessage = *message;
    return result;
}

ValueBindingSubscription HTMLInputElement::observeValueState(ValueStateObserver observer) {
    const std::size_t id = mNextValueObserver++;
    mValueObservers.emplace(id, std::move(observer));
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    return ValueBindingSubscription([this, lifetime, id] {
        if (!lifetime.expired()) mValueObservers.erase(id);
    });
}

void HTMLInputElement::notifyValueState() {
    const InputValueState state = valueState();
    const auto observers = mValueObservers;
    const std::weak_ptr<char> lifetime = mValueObserverLifetime;
    for (const auto& [id, observer] : observers) {
        if (lifetime.expired()) return;
        if (mValueObservers.find(id) != mValueObservers.end()) observer(state);
    }
}

void HTMLInputElement::applyValueState(ValueState<bool> state) {
    const ElementRef<HTMLInputElement> self(this);
    const bool changed = mValueState != state;
    mValueState = std::move(state);
    updateCheckedState(mValueState.value);
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
    if (!self) return;
    if (changed) notifyValueState();
}

void HTMLInputElement::synchronizeValueBinding() {
    if (mBinding) applyValueState(mBinding->state());
}

void HTMLInputElement::prepareValueBinding(Binder& binder) {
    if (mValueBindingRequest) binder.requireValueBinding(*mValueBindingRequest, mBinding);
}

ValueBindingSubscription HTMLInputElement::commitValueBinding(const std::shared_ptr<bool>& bindingActive, Element& root) {
    if (!mBinding) return {};
    std::weak_ptr<char> lifetime = mValueObserverLifetime;
    const std::weak_ptr<bool> active = bindingActive;
    const ElementRef<Element> rootRef(&root);
    const ElementRef<HTMLInputElement> inputRef(this);
    std::shared_ptr<ValueBinding<bool>> provider = mBinding.shared();
    const ValueBinding<bool>* providerPointer = provider.get();
    const auto isValid = [&] {
        HTMLInputElement* input = inputRef.get();
        Element* rootElement = rootRef.get();
        if (!input || !rootElement || input->mBinding.shared() != provider) return false;
        for (Node* current = input; current; current = current->parentNode())
            if (current == rootElement) return true;
        return false;
    };
    applyValueState(provider->state());
    if (!isValid()) return {};
    auto providerSubscription = std::make_shared<ValueBindingSubscription>(provider->observe([lifetime, active, rootRef, inputRef,
                                                                                              providerPointer](const ValueState<bool>& state) {
        const std::shared_ptr<bool> bindingIsActive = active.lock();
        Element* rootElement = rootRef.get();
        HTMLInputElement* input = inputRef.get();
        if (!lifetime.expired() && bindingIsActive && *bindingIsActive && rootElement && input && input->mBinding.shared().get() == providerPointer) {
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
        if (!lifetime.expired() && mBinding.shared() == provider) mBinding.reset();
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
        if (!current || current->mBinding.shared() != provider) return;
        current->applyValueState(provider->state());
    } else {
        mValueState.value = checked;
        updateCheckedState(checked);
        if (isRadioType()) updateRadioGroup();
        else refreshIndeterminateState();
        notifyValueState();
    }
    HTMLInputElement* current = self.get();
    if (!current || current->checked() == previous) return;
    if (current->mOnCheckedChanged) current->mOnCheckedChanged(current->checked());
    current = self.get();
    if (!current) return;
    Event input(kInputEvent, *current, current->checked());
    current->dispatchEvent(input);
    current = self.get();
    if (!current) return;
    Event change(kChangeEvent, *current, current->checked());
    current->dispatchEvent(change);
}

void HTMLInputElement::onTreeAttached() {
    if (isRadioType()) updateRadioGroup();
    else refreshIndeterminateState();
}

void HTMLInputElement::onTreeWillBeDetached() {
    if (isRadioType()) refreshRadioGroup(mName, this);
}

void HTMLInputElement::onActivate() {
    const std::string key = canonicalizeHTMLName(mType);
    if (key == "checkbox") {
        indeterminate(false);
        if (isSwitchType()) activateSwitch();
        else activateCheckbox();
    } else if (key == "radio") activateRadio();
}

std::vector<PseudoElement*> HTMLInputElement::generatedPseudoElements() const {
    if (isSwitchType()) return {&mSliderTrack, &mSliderThumb};
    if (isCheckableType(mType)) return {&mCheckmark};
    return {};
}
} // namespace radia::ui
