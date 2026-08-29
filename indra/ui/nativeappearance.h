/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <memory>
#include <optional>
#include "path.h"
#include "style/style.h"
#include "surface/scrollgeometry.h"

namespace radia::ui {
struct NativeScrollbarMetrics {
    float thickness;
    float minimumThumbLength;
    float arrowLength;
    float thumbPadding;
};

struct NativeScrollbarState {
    ScrollbarPart hoveredPart = ScrollbarPart::NoneValue;
    ScrollbarPart pressedPart = ScrollbarPart::NoneValue;
    bool disabled = false;
};

struct NativeScrollbarClip {
    bool enabled = false;
    Rect borderBox;
    BorderRadii borderRadius;
    EdgeInsets borderWidth;
};

struct NativeScrollbarPaintRequest {
    ScrollGeometry geometry;
    NativeScrollbarMetrics metrics{};
    ScrollbarColors colors;
    NativeScrollbarClip clip;
    ScrollbarMode mode = ScrollbarMode::Classic;
    LayoutDirection direction = LayoutDirection::LeftToRight;
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

enum class NativeInputControl : std::uint8_t { Checkbox, Radio, Switch };

enum class NativeInputMark : std::uint8_t { Check, Dash };

struct NativeInputMarkPaintRequest {
    NativeInputMark mark = NativeInputMark::Check;
    Rect bounds;
    Color color;
    float strokeWidth = 0.f;
    float radius = 0.f;
    float scale = 1.f;
    Path path;
};

class NativeControlPaintContext {
public:
    virtual ~NativeControlPaintContext() = default;
    virtual void paintNativeBox(const Rect& rect, const Style& style) = 0;
    virtual void paintNativeInputMark(const NativeInputMarkPaintRequest&) = 0;
};

struct NativeInputMetrics {
    Vec2 intrinsicSize;
};

struct NativeInputPaintRequest {
    NativeInputControl control = NativeInputControl::Checkbox;
    Rect bounds;
    bool checked = false;
    bool indeterminate = false;
    bool disabled = false;
    bool hovered = false;
    bool pressed = false;
    std::optional<Color> accentColor;
    ColorScheme colorScheme = ColorScheme::Auto;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    float scale = 1.f;
};

struct NativeButtonPaintRequest {
    Rect bounds;
    Style style;
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
} // namespace radia::ui
