/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <optional>
#include "html/button.h"
#include "html/element.h"

namespace radia::ui {
class HTMLMinimizeButtonElement final : public HTMLButtonElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

private:
    HTMLMinimizeButtonElement();
};

class HTMLCloseButtonElement final : public HTMLButtonElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

private:
    HTMLCloseButtonElement();
};

class HTMLFloaterElement : public HTMLElement {
    friend class Surface;
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

public:
    std::string title() const;
    bool closable() const { return mClosable; }
    bool minimizable() const { return mMinimizable; }
    bool resizeable() const { return mResizeable; }
    bool closed() const { return mClosed; }
    bool minimized() const { return mMinimized; }
    bool dragging() const { return mInteraction == FloaterInteraction::Move; }
    const Rect& expandedRect() const { return mExpandedRect; }
    Vec2 authoredSize() const;
    Vec2 authoredContentSize() const { return mAuthoredContentSize; }

    Element* head() { return mHead; }
    const Element* head() const { return mHead; }
    Element* body() { return mBody; }
    const Element* body() const { return mBody; }
    HTMLButtonElement* closeButton() { return mCloseButton; }
    const HTMLButtonElement* closeButton() const { return mCloseButton; }
    HTMLButtonElement* minimizeButton() { return mMinimizeButton; }
    const HTMLButtonElement* minimizeButton() const { return mMinimizeButton; }

    HTMLFloaterElement& setLifecycleCallbacks(std::function<void()> onOpen, std::function<void()> onClose);
    void open();
    void close();
    void setMinimized(bool minimized);
    void toggleMinimized();
    HTMLFloaterElement& setResizeable(bool value);

    bool defaultPointerEvents() const override { return true; }

protected:
    bool beginPointerInteraction(const PointerEvent& event) override;
    bool updatePointerInteraction(const PointerEvent& event) override;
    bool endPointerInteraction(const PointerEvent& event) override;
    void onChildAdded(Element& child) override;
    void onChildRemoved(Element& child) override;
    void onDescendantAdded(Element& child) override;
    void onDescendantRemoved(Element& child) override;
    void onChildrenCleared() override;
    void onLocaleChanged(const System& system) override;

private:
    HTMLFloaterElement();

    enum class FloaterInteraction : std::uint8_t { Idle, Move, Resize };

    struct ResizeInteraction {
        std::uint8_t edges = 0;
        Vec2 initialPointer;
        Rect initialRect;
        Vec2 minimum;
        std::optional<Rect> bounds;
    };

    bool overChromeButton(const Vec2& point) const;
    Vec2 clampedPosition(const Vec2& position) const;
    bool beginResizeInteraction(const PointerEvent& event, std::uint8_t edges, const Vec2& minimum, const std::optional<Rect>& bounds);
    void setAuthoredSize(const Vec2& size, const Vec2& contentSize);
    void refreshAuthoredStructure();
    void setMovementBounds(const Rect& bounds);
    void clampToMovementBounds();

    Rect mMovementBounds;
    Rect mExpandedRect;
    Vec2 mDragOffset;
    Vec2 mAuthoredSize;
    Vec2 mAuthoredContentSize;
    ResizeInteraction mResizeInteraction;
    Element* mHead = nullptr;
    Element* mBody = nullptr;
    Element* mTitleElement = nullptr;
    HTMLButtonElement* mCloseButton = nullptr;
    HTMLButtonElement* mMinimizeButton = nullptr;
    bool mClosable = false;
    bool mMinimizable = false;
    bool mResizeable = false;
    bool mClosed = false;
    bool mMinimized = false;
    FloaterInteraction mInteraction = FloaterInteraction::Idle;
    bool mAuthoredSizeCaptured = false;
    std::function<void()> mOnOpen;
    std::function<void()> mOnClose;
};
} // namespace radia::ui
