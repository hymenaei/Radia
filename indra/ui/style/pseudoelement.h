/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <string_view>
#include <utility>
#include <vector>
#include "style/computedstyle.h"
#include "types.h"

namespace radia::ui {
class Element;
class HTMLInputElement;
class LayoutEngine;
class StylePass;

enum class PseudoElementType : uint8_t { Checkmark, SliderTrack, SliderFill, SliderThumb };

constexpr std::string_view pseudoElementName(PseudoElementType type) {
    switch (type) {
        case PseudoElementType::Checkmark: return "checkmark";
        case PseudoElementType::SliderTrack: return "slider-track";
        case PseudoElementType::SliderFill: return "slider-fill";
        case PseudoElementType::SliderThumb: return "slider-thumb";
    }
    return {};
}

class PseudoElement final {
public:
    PseudoElement(PseudoElementType type, Element& originatingElement, PseudoElement* parent = nullptr)
        : mType(type), mOriginatingElement(&originatingElement), mParent(parent) {}

    PseudoElementType type() const noexcept { return mType; }
    std::string_view name() const noexcept { return pseudoElementName(mType); }
    const Element& originatingElement() const noexcept { return *mOriginatingElement; }
    PseudoElement* parentPseudoElement() noexcept { return mParent; }
    const PseudoElement* parentPseudoElement() const noexcept { return mParent; }
    const std::vector<PseudoElement*>& generatedPseudoElements() const noexcept { return mGenerated; }
    const Rect& rect() const noexcept { return mRect; }
    const Vec2& desiredSize() const noexcept { return mDesiredSize; }
    const ComputedStyle& style() const noexcept { return mStyle; }

private:
    friend class Element;
    friend class HTMLInputElement;
    friend class LayoutEngine;
    friend class StylePass;

    void addGeneratedPseudoElement(PseudoElement& pseudoElement) { mGenerated.push_back(&pseudoElement); }
    void setResolvedStyle(ComputedStyle style) { mStyle = std::move(style); }
    void setDesiredSize(Vec2 size) { mDesiredSize = std::move(size); }
    void setRect(const Rect& rect) { mRect = rect; }
    void translate(const Vec2& delta) {
        mRect.x += delta.x;
        mRect.y += delta.y;
        for (PseudoElement* pseudoElement : mGenerated)
            if (pseudoElement) pseudoElement->translate(delta);
    }
    PseudoElementType mType;
    Element* mOriginatingElement = nullptr;
    PseudoElement* mParent = nullptr;
    std::vector<PseudoElement*> mGenerated;
    ComputedStyle mStyle;
    Rect mRect;
    Vec2 mDesiredSize;
};
} // namespace radia::ui
