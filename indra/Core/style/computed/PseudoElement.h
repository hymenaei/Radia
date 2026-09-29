/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string_view>
#include <utility>
#include <vector>
#include "CSSPseudoSelectors.h"
#include "ComputedStyle.h"
#include "LayoutGeometry.h"

namespace Core {
class Element;
class HTMLInputElement;
} // namespace Core

namespace Core::Layout {
class Engine;
} // namespace Core::Layout

namespace Core::Style {
class Pass;

class PseudoElement final {
public:
    PseudoElement(CSS::PseudoElement type, Element& originatingElement, PseudoElement* parent = nullptr)
        : mType(type)
        , mOriginatingElement(&originatingElement)
        , mParent(parent) {}

    CSS::PseudoElement type() const noexcept { return mType; }
    std::string_view name() const noexcept { return CSS::pseudoElementName(mType); }
    const Element& originatingElement() const noexcept { return *mOriginatingElement; }
    PseudoElement* parentPseudoElement() noexcept { return mParent; }
    const PseudoElement* parentPseudoElement() const noexcept { return mParent; }
    const std::vector<PseudoElement*>& generatedPseudoElements() const noexcept { return mGenerated; }
    const Layout::Rect& rect() const noexcept { return mRect; }
    const Layout::Vec2& desiredSize() const noexcept { return mDesiredSize; }
    const ComputedStyle& style() const noexcept { return mStyle; }

private:
    friend class Element;
    friend class HTMLInputElement;
    friend class Layout::Engine;
    friend class Pass;

    void addGeneratedPseudoElement(PseudoElement& pseudoElement) { mGenerated.push_back(&pseudoElement); }
    void setResolvedStyle(ComputedStyle style) { mStyle = std::move(style); }
    void setDesiredSize(Layout::Vec2 size) { mDesiredSize = std::move(size); }
    void setRect(const Layout::Rect& rect) { mRect = rect; }
    void translate(const Layout::Vec2& delta) {
        mRect.x += delta.x;
        mRect.y += delta.y;
        for (PseudoElement* pseudoElement : mGenerated)
            if (pseudoElement)
                pseudoElement->translate(delta);
    }
    CSS::PseudoElement mType;
    Element* mOriginatingElement = nullptr;
    PseudoElement* mParent = nullptr;
    std::vector<PseudoElement*> mGenerated;
    ComputedStyle mStyle;
    Layout::Rect mRect;
    Layout::Vec2 mDesiredSize;
};
} // namespace Core::Style
