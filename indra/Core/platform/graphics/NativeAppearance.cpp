/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "NativeAppearance.h"
#include <algorithm>
#include <utility>
#include "PaintContext.h"

namespace Core {
using enum CSS::KeywordName;

namespace {
const NativeScrollbarMetrics kDefaultScrollbarMetrics {15.f, 20.f, 15.f, 3.f};
const NativeInputMetrics kCheckboxMetrics {{13.f, 13.f}};
const NativeInputMetrics kRadioMetrics {{13.f, 13.f}};
const NativeInputMetrics kSwitchMetrics {{36.f, 20.f}};

const Color kDefaultScrollbarTrackColor(0.08f, 0.09f, 0.1f, 0.52f);
const Color kDefaultScrollbarThumbColor(0.55f, 0.58f, 0.62f, 0.88f);

struct HslColor {
    float h = 0.f;
    float s = 0.f;
    float l = 0.f;
};

HslColor toHsl(const Color& color) {
    const float red = std::clamp(color.r, 0.f, 1.f);
    const float green = std::clamp(color.g, 0.f, 1.f);
    const float blue = std::clamp(color.b, 0.f, 1.f);
    const float maximum = std::max({red, green, blue});
    const float minimum = std::min({red, green, blue});
    const float delta = maximum - minimum;

    HslColor result;
    result.l = (maximum + minimum) * .5f;
    if (delta == 0.f)
        return result;

    result.s = result.l > .5f ? delta / (2.f - maximum - minimum) : delta / (maximum + minimum);
    if (maximum == red)
        result.h = (green - blue) / delta + (green < blue ? 6.f : 0.f);
    else if (maximum == green)
        result.h = (blue - red) / delta + 2.f;
    else
        result.h = (red - green) / delta + 4.f;
    result.h /= 6.f;
    return result;
}

float hueToRgb(float p, float q, float t) {
    if (t < 0.f)
        t += 1.f;
    if (t > 1.f)
        t -= 1.f;
    if (t < 1.f / 6.f)
        return p + (q - p) * 6.f * t;
    if (t < .5f)
        return q;
    if (t < 2.f / 3.f)
        return p + (q - p) * (2.f / 3.f - t) * 6.f;
    return p;
}

Color fromHsl(const HslColor& hsl, float alpha) {
    if (hsl.s == 0.f)
        return {hsl.l, hsl.l, hsl.l, alpha};
    const float q = hsl.l < .5f ? hsl.l * (1.f + hsl.s) : hsl.l + hsl.s - hsl.l * hsl.s;
    const float p = 2.f * hsl.l - q;
    return {hueToRgb(p, q, hsl.h + 1.f / 3.f), hueToRgb(p, q, hsl.h), hueToRgb(p, q, hsl.h - 1.f / 3.f), alpha};
}

Color inputAccent(const NativeInputPaintRequest& request) {
    if (request.disabled) {
        Color color = systemColorValue(KeywordGrayText, request.colorScheme);
        color.a = .55f;
        return color;
    }
    const Color accent = request.accentColor.value_or(systemColorValue(KeywordAccentColor, request.colorScheme));
    if (!request.pressed && !request.hovered)
        return accent;
    HslColor adjusted = toHsl(accent);
    float lightnessAdjustment = request.pressed ? .11f : -.11f;
    if (request.colorScheme == Style::ColorSchemeMode::Dark)
        lightnessAdjustment = -lightnessAdjustment;
    adjusted.l = std::clamp(adjusted.l + lightnessAdjustment, 0.f, 1.f);
    return fromHsl(adjusted, accent.a);
}

Color inputMarkColor(const NativeInputPaintRequest& request) {
    Color color = systemColorValue(request.disabled ? KeywordGrayText : KeywordAccentColorText, request.colorScheme);
    if (request.disabled)
        color.a = .6f;
    return color;
}

Color scrollbarStateColor(Color color, const NativeScrollbarState& state, ScrollbarPart part) {
    if (state.disabled)
        return color.withAlpha(color.a * .55f);
    const float amount = state.pressedPart == part ? .16f : state.hoveredPart == part ? .12f : 0.f;
    if (amount == 0.f)
        return color;
    const Color target = state.pressedPart == part ? Color(0.f, 0.f, 0.f, color.a) : Color(1.f, 1.f, 1.f, color.a);
    return {color.r + (target.r - color.r) * amount, color.g + (target.g - color.g) * amount, color.b + (target.b - color.b) * amount,
        color.a};
}

Color nativeControlBackground(const NativeInputPaintRequest& request) {
    Color color = systemColorValue(KeywordField, request.colorScheme);
    if (request.disabled)
        color.a = .6f;
    return color;
}

Color nativeControlBorder(const NativeInputPaintRequest& request) {
    Color color = systemColorValue(request.disabled ? KeywordGrayText : KeywordButtonBorder, request.colorScheme);
    if (request.disabled) {
        color.a = .6f;
        return color;
    }
    if (!request.pressed && !request.hovered)
        return color;

    HslColor adjusted = toHsl(color);
    const float contrastAdjustment = (request.colorScheme == Style::ColorSchemeMode::Dark ? 1.f : -1.f) * (request.pressed ? .2f : .12f);
    adjusted.l = std::clamp(adjusted.l + contrastAdjustment, 0.f, 1.f);
    return fromHsl(adjusted, color.a);
}

Color switchTrack(const NativeInputPaintRequest& request) { return request.checked ? inputAccent(request) : nativeControlBorder(request); }

Color switchThumb(const NativeInputPaintRequest& request) { return nativeControlBackground(request); }

Style::ComputedStyle nativeControlStyle(Color background, Color border, float radius, Style::ColorSchemeMode colorScheme,
    bool bordered = true) {
    Style::ComputedStyle result;
    result.usedColorScheme = colorScheme;
    result.setBackgroundColor(std::move(background));
    result.setBorderColor(std::move(border));
    result.setBorderStyle(bordered ? Style::BorderStyle::Solid : Style::BorderStyle::NoneValue);
    result.setBorderWidth(Style::LineWidth {bordered ? 1.f : 0.f});
    result.setBorderRadius(Style::BorderRadius::uniform(Style::Length {radius}));
    return result;
}

Style::ComputedStyle nativeMarkStyle(Color color, Style::ColorSchemeMode colorScheme, float radius = 0.f) {
    Style::ComputedStyle result;
    result.usedColorScheme = colorScheme;
    result.setBackgroundColor(Color {color});
    result.setBorderColor(std::move(color));
    result.setBorderRadius(Style::BorderRadius::uniform(Style::Length {radius}));
    return result;
}

void paintNativeInputBox(NativeControlPaintContext& context, const Layout::Rect& rect, Style::ComputedStyle style, float opacity) {
    resolveStyleColors(style, systemColorValue(KeywordCanvasText, style.usedColorScheme));
    applyOpacity(style, opacity);
    context.paintNativeBox(rect, style);
}

Layout::Rect centeredSquare(const Layout::Rect& bounds) {
    const float size = std::min(bounds.w, bounds.h);
    return {bounds.x + (bounds.w - size) * .5f, bounds.y + (bounds.h - size) * .5f, std::max(0.f, size), std::max(0.f, size)};
}

void paintInputBase(NativeControlPaintContext& context, const Layout::Rect& bounds, Color background, Color border, float radius,
    bool drawBorder, float opacity, Style::ColorSchemeMode colorScheme) {
    const Layout::Rect backgroundBounds = Layout::insetRect(bounds, {.2f, .2f, .2f, .2f});
    paintNativeInputBox(context, backgroundBounds, nativeMarkStyle(background, colorScheme, std::max(0.f, radius - .2f)), opacity);
    if (!drawBorder)
        return;

    Style::ComputedStyle borderStyle = nativeMarkStyle(Color(0.f, 0.f, 0.f, 0.f), colorScheme, radius);
    borderStyle.setBorderColor(std::move(border));
    borderStyle.setBorderStyle(Style::BorderStyle::Solid);
    borderStyle.setBorderWidth(Style::LineWidth {1.f});
    paintNativeInputBox(context, bounds, borderStyle, opacity);
}

void paintSwitch(NativeControlPaintContext& context, const NativeInputPaintRequest& request) {
    const Layout::Rect bounds = request.bounds;
    const float radius = std::max(0.f, bounds.h * .5f);
    const Color track = switchTrack(request);
    paintNativeInputBox(context, bounds, nativeControlStyle(track, track, radius, request.colorScheme), request.opacity);

    const float inset = std::min(2.f, std::max(0.f, std::min(bounds.w, bounds.h) * .5f));
    const float thumbSize = std::max(0.f, bounds.h - inset * 2.f);
    const bool thumbAtRight = request.direction == Layout::Direction::LeftToRight ? request.checked : !request.checked;
    const float thumbLeft = thumbAtRight ? bounds.right() - inset - thumbSize : bounds.left() + inset;
    const Layout::Rect thumb {thumbLeft, bounds.y + inset, thumbSize, thumbSize};
    paintNativeInputBox(context, thumb, nativeMarkStyle(switchThumb(request), request.colorScheme, thumbSize * .5f), request.opacity);
}

void paintCheckbox(NativeControlPaintContext& context, const NativeInputPaintRequest& request) {
    const bool selected = request.checked || request.indeterminate;
    const Layout::Rect bounds = centeredSquare(request.bounds);
    const float radius = std::min(2.f, std::min(bounds.w, bounds.h) * .5f);
    const Color accent = inputAccent(request);
    paintInputBase(context, bounds, nativeControlBackground(request), nativeControlBorder(request), radius, !selected, request.opacity,
        request.colorScheme);
    if (selected)
        paintNativeInputBox(context, bounds, nativeMarkStyle(accent, request.colorScheme, radius), request.opacity);
    if (!selected)
        return;

    NativeInputMarkPaintRequest mark;
    mark.mark = request.indeterminate ? NativeInputMark::Dash : NativeInputMark::Check;
    const Color markColor = inputMarkColor(request);
    mark.color = markColor.withAlpha(markColor.a * request.opacity);
    mark.scale = request.scale;
    if (request.indeterminate) {
        const float xInset = bounds.w * (5.5f / 13.f);
        const float yInset = bounds.h * (5.5f / 13.f);
        mark.bounds = {bounds.x + xInset, bounds.y + yInset, bounds.w - xInset * 2.f, bounds.h - yInset * 2.f};
        mark.radius = radius;
    } else {
        mark.bounds = bounds;
        mark.strokeWidth = bounds.h * .16f;
        mark.path.moveTo(bounds.x + bounds.w * .2f, bounds.y + bounds.h * .5f)
            .lineTo(bounds.x + bounds.w * .4f, bounds.y + bounds.h * .3f)
            .lineTo(bounds.x + bounds.w * .8f, bounds.y + bounds.h * .8f);
    }
    context.paintNativeInputMark(mark);
}

void paintRadio(NativeControlPaintContext& context, const NativeInputPaintRequest& request) {
    const Layout::Rect bounds = centeredSquare(request.bounds);
    const float radius = std::min(bounds.w, bounds.h) * .5f;
    const Color accent = inputAccent(request);
    paintInputBase(context, bounds, nativeControlBackground(request), request.checked ? accent : nativeControlBorder(request), radius, true,
        request.opacity, request.colorScheme);
    if (!request.checked)
        return;

    const float inset = std::min(bounds.w, bounds.h) * .2f;
    const float size = std::max(0.f, std::min(bounds.w, bounds.h) - inset * 2.f);
    const Layout::Rect dot {bounds.x + (bounds.w - size) * .5f, bounds.y + (bounds.h - size) * .5f, size, size};
    paintNativeInputBox(context, dot, nativeMarkStyle(accent, request.colorScheme, size * .5f), request.opacity);
}
} // namespace

NativeLayoutMetrics defaultNativeLayoutMetrics() noexcept {
    return {kDefaultScrollbarMetrics, kDefaultScrollbarMetrics, kCheckboxMetrics, kRadioMetrics, kSwitchMetrics, 1};
}

NativeLayoutMetrics NativeAppearance::layoutMetrics() const {
    return {scrollbarMetrics(ScrollbarMode::Classic), scrollbarMetrics(ScrollbarMode::Overlay), inputMetrics(NativeInputControl::Checkbox),
        inputMetrics(NativeInputControl::Radio), inputMetrics(NativeInputControl::Switch), revision()};
}

NativeAppearanceBase::NativeAppearanceBase()
    : mScrollbarMetrics(kDefaultScrollbarMetrics) {}

NativeAppearanceBase::NativeAppearanceBase(NativeScrollbarMetrics metrics)
    : mScrollbarMetrics(metrics) {}

NativeScrollbarMetrics NativeAppearanceBase::scrollbarMetrics(ScrollbarMode) const { return mScrollbarMetrics; }

NativeScrollbarPaintStyle NativeAppearanceBase::scrollbarPaintStyle(const NativeScrollbarPaintRequest& request, ScrollbarAxis axis) const {
    const Color track = request.colors.automatic ? kDefaultScrollbarTrackColor : request.colors.track.resolvedColor();
    const Color baseThumb = request.colors.automatic ? kDefaultScrollbarThumbColor : request.colors.thumb.resolvedColor();
    const NativeScrollbarState& state = axis == ScrollbarAxis::Vertical ? request.vertical : request.horizontal;
    const NativeScrollbarAxisGeometry& geometry = axis == ScrollbarAxis::Vertical ? request.geometry.vertical : request.geometry.horizontal;
    const Color glyph = baseThumb.withAlpha(std::min(1.f, baseThumb.a + .05f));
    return {track, scrollbarStateColor(baseThumb, state, ScrollbarPart::Thumb),
        scrollbarStateColor(glyph, state, ScrollbarPart::StartArrow), scrollbarStateColor(glyph, state, ScrollbarPart::EndArrow),
        std::min(geometry.thumb.w, geometry.thumb.h) * .5f};
}

NativeInputMetrics NativeAppearanceBase::inputMetrics(NativeInputControl control) const {
    switch (control) {
    case NativeInputControl::Checkbox:
        return kCheckboxMetrics;
    case NativeInputControl::Radio:
        return kRadioMetrics;
    case NativeInputControl::Switch:
        return kSwitchMetrics;
    }
    return {};
}

void NativeAppearanceBase::paintInput(NativeControlPaintContext& context, const NativeInputPaintRequest& request) const {
    switch (request.control) {
    case NativeInputControl::Checkbox:
        paintCheckbox(context, request);
        break;
    case NativeInputControl::Radio:
        paintRadio(context, request);
        break;
    case NativeInputControl::Switch:
        paintSwitch(context, request);
        break;
    }
}

void NativeAppearanceBase::paintButton(NativeControlPaintContext& context, const NativeButtonPaintRequest& request) const {
    context.paintNativeBox(request.bounds, request.style);
}

const NativeAppearance& defaultNativeAppearance() {
    static const NativeAppearanceBase appearance;
    return appearance;
}

std::shared_ptr<const NativeAppearance> makeDefaultNativeAppearance() { return std::make_shared<NativeAppearanceBase>(); }
} // namespace Core
