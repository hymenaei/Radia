/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <memory>
#include <optional>
#include "ComputedStyle.h"
#include "LayoutGeometry.h"
#include "Path.h"

namespace Core {
enum class ScrollbarMode : std::uint8_t {
    Classic,
    Overlay
};
enum class ScrollbarAxis : std::uint8_t {
    NoneValue,
    Horizontal,
    Vertical
};
enum class ScrollbarPart : std::uint8_t {
    NoneValue,
    Track,
    Thumb,
    StartArrow,
    EndArrow,
    Corner
};

struct NativeScrollbarMetrics {
    float thickness = 0.f;
    float minimumThumbLength = 0.f;
    float arrowLength = 0.f;
    float thumbPadding = 0.f;

    friend constexpr bool operator==(const NativeScrollbarMetrics&, const NativeScrollbarMetrics&) = default;
};

enum class NativeInputControl : std::uint8_t {
    Checkbox,
    Radio,
    Switch
};

struct NativeInputMetrics {
    Layout::Vec2 intrinsicSize;

    friend constexpr bool operator==(const NativeInputMetrics& left, const NativeInputMetrics& right) {
        return left.intrinsicSize.x == right.intrinsicSize.x && left.intrinsicSize.y == right.intrinsicSize.y;
    }
};

struct NativeLayoutMetrics {
    NativeScrollbarMetrics classicScrollbar;
    NativeScrollbarMetrics overlayScrollbar;
    NativeInputMetrics checkbox;
    NativeInputMetrics radio;
    NativeInputMetrics switchControl;
    std::uint64_t revision = 1;

    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const {
        return mode == ScrollbarMode::Classic ? classicScrollbar : overlayScrollbar;
    }

    NativeInputMetrics inputMetrics(NativeInputControl control) const {
        switch (control) {
        case NativeInputControl::Checkbox:
            return checkbox;
        case NativeInputControl::Radio:
            return radio;
        case NativeInputControl::Switch:
            return switchControl;
        }
        return {};
    }

    friend constexpr bool operator==(const NativeLayoutMetrics&, const NativeLayoutMetrics&) = default;
};

NativeLayoutMetrics defaultNativeLayoutMetrics() noexcept;

struct NativeScrollbarState {
    ScrollbarPart hoveredPart = ScrollbarPart::NoneValue;
    ScrollbarPart pressedPart = ScrollbarPart::NoneValue;
    bool disabled = false;
};

struct NativeScrollbarClip {
    bool enabled = false;
    Layout::Rect borderBox;
    Style::BorderRadius borderRadius;
    Layout::RectEdges<float> borderWidth;
};

struct NativeScrollbarAxisGeometry {
    ScrollbarAxis axis = ScrollbarAxis::NoneValue;
    Layout::Rect bounds;
    Layout::Rect track;
    Layout::Rect thumb;
    Layout::Rect startArrow;
    Layout::Rect endArrow;
    bool visible = false;
    bool reversed = false;
};

struct NativeScrollbarPaintGeometry {
    NativeScrollbarAxisGeometry horizontal;
    NativeScrollbarAxisGeometry vertical;
    Layout::Rect corner;
    bool hasCorner = false;
};

struct NativeScrollbarPaintRequest {
    NativeScrollbarPaintGeometry geometry;
    NativeScrollbarMetrics metrics {};
    Style::ScrollbarColor colors;
    NativeScrollbarClip clip;
    ScrollbarMode mode = ScrollbarMode::Classic;
    Layout::Direction direction = Layout::Direction::LeftToRight;
    NativeScrollbarState horizontal;
    NativeScrollbarState vertical;
    float scale = 1.f;
    std::uint64_t appearanceRevision = 1;
};

struct NativeScrollbarPaintStyle {
    Color track;
    Color thumb;
    Color startArrow;
    Color endArrow;
    float thumbRadius = 0.f;
};

enum class NativeInputMark : std::uint8_t {
    Check,
    Dash
};

struct NativeInputMarkPaintRequest {
    NativeInputMark mark = NativeInputMark::Check;
    Layout::Rect bounds;
    Color color;
    float strokeWidth = 0.f;
    float radius = 0.f;
    float scale = 1.f;
    Path path;
};

class NativeControlPaintContext {
public:
    virtual ~NativeControlPaintContext() = default;
    virtual void paintNativeBox(const Layout::Rect& rect, const Style::ComputedStyle& style) = 0;
    virtual void paintNativeInputMark(const NativeInputMarkPaintRequest&) = 0;
};

struct NativeInputPaintRequest {
    NativeInputControl control = NativeInputControl::Checkbox;
    Layout::Rect bounds;
    bool checked = false;
    bool indeterminate = false;
    bool disabled = false;
    bool hovered = false;
    bool pressed = false;
    float opacity = 1.f;
    std::optional<Color> accentColor;
    Style::ColorSchemeMode colorScheme = Style::ColorSchemeMode::Dark;
    Layout::Direction direction = Layout::Direction::LeftToRight;
    float scale = 1.f;
};

struct NativeButtonPaintRequest {
    Layout::Rect bounds;
    Style::ComputedStyle style;
    bool disabled = false;
    bool hovered = false;
    bool pressed = false;
    bool focused = false;
    bool focusVisible = false;
    float scale = 1.f;
};

class NativeAppearance {
public:
    virtual ~NativeAppearance() = default;
    virtual NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode) const = 0;
    virtual NativeLayoutMetrics layoutMetrics() const;
    virtual NativeScrollbarPaintStyle scrollbarPaintStyle(const NativeScrollbarPaintRequest&, ScrollbarAxis) const = 0;
    virtual NativeInputMetrics inputMetrics(NativeInputControl) const = 0;
    virtual void paintInput(NativeControlPaintContext&, const NativeInputPaintRequest&) const = 0;
    virtual void paintButton(NativeControlPaintContext&, const NativeButtonPaintRequest&) const = 0;
    virtual std::uint64_t revision() const noexcept { return 1; }
};

class NativeAppearanceBase : public NativeAppearance {
public:
    NativeAppearanceBase();
    explicit NativeAppearanceBase(NativeScrollbarMetrics metrics);

    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode) const override;
    NativeScrollbarPaintStyle scrollbarPaintStyle(const NativeScrollbarPaintRequest&, ScrollbarAxis) const override;
    NativeInputMetrics inputMetrics(NativeInputControl) const override;
    void paintInput(NativeControlPaintContext&, const NativeInputPaintRequest&) const override;
    void paintButton(NativeControlPaintContext&, const NativeButtonPaintRequest&) const override;

private:
    NativeScrollbarMetrics mScrollbarMetrics;
};

const NativeAppearance& defaultNativeAppearance();
std::shared_ptr<const NativeAppearance> makeDefaultNativeAppearance();
} // namespace Core
