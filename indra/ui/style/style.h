/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
#include "types.h"

namespace radia::ui {
struct Length {
    float pixels = 0.f;
    float percent = 0.f;

    float resolve(float reference) const { return pixels + percent * reference; }
    bool isPercentage() const { return percent != 0.f; }
};

struct BorderRadius {
    Length horizontal;
    Length vertical;

    static BorderRadius uniform(Length radius) { return {radius, radius}; }
};

struct BorderRadii {
    BorderRadius topLeft;
    BorderRadius topRight;
    BorderRadius bottomRight;
    BorderRadius bottomLeft;

    static BorderRadii uniform(Length radius) {
        const BorderRadius corner = BorderRadius::uniform(radius);
        return {corner, corner, corner, corner};
    }
};

struct LightDarkColor {
    Color light;
    Color dark;
};

struct ScrollbarColors {
    bool automatic = true;
    Color thumb;
    Color track;
    std::optional<LightDarkColor> thumbLightDarkColor;
    std::optional<LightDarkColor> trackLightDarkColor;
};

struct AccentColor {
    enum class Kind : uint8_t { Auto, CurrentColor, Color };

    Kind kind = Kind::Auto;
    Color color;
    std::optional<LightDarkColor> lightDarkColor;

    static AccentColor currentColor() { return {Kind::CurrentColor, {}}; }
    static AccentColor fromColor(Color value) { return {Kind::Color, value}; }
    static AccentColor fromLightDark(LightDarkColor value) { return {Kind::Color, value.dark, std::move(value)}; }
};

class Dimension {
public:
    static Dimension fromPixels(float pixels) {
        Dimension result;
        result.mLength = Length{pixels};
        return result;
    }

    static Dimension fromLength(Length length) {
        Dimension result;
        result.mLength = length;
        return result;
    }

    bool isAuto() const { return !mLength.has_value(); }
    float pixels() const { return mLength.value().pixels; }
    bool isPercentage() const { return mLength && mLength->isPercentage(); }
    float resolve(float fallback, float reference = 0.f) const { return mLength ? mLength->resolve(reference) : fallback; }

private:
    std::optional<Length> mLength;
};

class MarginValue {
public:
    MarginValue() = default;

    static MarginValue automatic() {
        MarginValue result;
        result.mAutomatic = true;
        return result;
    }

    static MarginValue fromPixels(float pixels) { return MarginValue(Length{pixels}); }

    bool isAuto() const { return mAutomatic; }
    float fixedPixels() const { return mAutomatic ? 0.f : mLength.pixels; }

private:
    explicit MarginValue(Length length) : mLength(length) {}

    bool mAutomatic = false;
    Length mLength;
};

class GapValue {
public:
    GapValue() = default;

    static GapValue automatic() {
        GapValue result;
        result.mAutomatic = true;
        return result;
    }

    static GapValue fromPixels(float pixels) {
        GapValue result;
        result.mPixels = pixels;
        return result;
    }

    bool isAuto() const { return mAutomatic; }
    float fixedPixels() const { return mAutomatic ? 0.f : mPixels; }

private:
    bool mAutomatic = false;
    float mPixels = 0.f;
};

struct MarginInsets {
    MarginValue top;
    MarginValue right;
    MarginValue bottom;
    MarginValue left;

    float horizontal() const { return left.fixedPixels() + right.fixedPixels(); }
    float vertical() const { return top.fixedPixels() + bottom.fixedPixels(); }
    int horizontalAutoCount() const { return static_cast<int>(left.isAuto()) + static_cast<int>(right.isAuto()); }
    int verticalAutoCount() const { return static_cast<int>(top.isAuto()) + static_cast<int>(bottom.isAuto()); }
};

struct GradientStop {
    Color color;
    float position = 0.f;
    std::optional<LightDarkColor> lightDarkColor;
};

enum class GradientKind { Linear, Radial, Conic };

enum class RadialGradientShape { Ellipse, Circle };

struct Gradient {
    GradientKind kind = GradientKind::Linear;
    bool repeating = false;
    float angleDegrees = 180.f;
    Vec2 center = {.5f, .5f};
    RadialGradientShape radialShape = RadialGradientShape::Ellipse;
    std::vector<GradientStop> stops;
};

struct BoxShadow {
    float horizontal = 0.f;
    float vertical = 0.f;
    float blur = 0.f;
    float spread = 0.f;
    Color color;
    bool inset = false;
    std::optional<LightDarkColor> lightDarkColor;
};

enum class OutlineStyle { Solid, Dashed };

struct Outline {
    float width = 0.f;
    float offset = 0.f;
    Color color;
    OutlineStyle style = OutlineStyle::Solid;
    std::optional<LightDarkColor> lightDarkColor;
};

enum class EffectKind { BackgroundBlur, LayerBlur };

struct Effect {
    EffectKind kind = EffectKind::LayerBlur;
    float startRadius = 0.f;
    float endRadius = 0.f;
    float startPosition = 0.f;
    float endPosition = 1.f;
    float angleDegrees = 180.f;

    bool progressive() const { return startRadius != endRadius; }
};

inline constexpr std::size_t kMaxEffectCount = 8;

struct GridArea {
    int row = 1;
    int column = 1;
};

struct Translate {
    float x = 0.f;
    float y = 0.f;
};

enum class AppearanceMode { Auto, Base, Unstyled };
enum class ColorScheme { Auto, Light, Dark, LightDark };
enum class BoxSizing { ContentBox, BorderBox };
enum class DisplayMode { Inline, InlineBlock, Block, Flex, InlineFlex, Grid, InlineGrid, NoneValue };

inline constexpr bool isFlexDisplay(DisplayMode display) noexcept {
    return display == DisplayMode::Flex || display == DisplayMode::InlineFlex;
}
enum class BorderStyle { Solid, Outset, Inset };
enum class FlexDirection { Row, Column };
enum class PositionMode { Static, Relative };
enum class JustifyContent { Start, Center, End, Left, Right };
enum class JustifySelf { Auto, Start, Center, End, Stretch };
enum class AlignItems { Normal, Start, Center, End, Stretch };
enum class AlignSelf { Auto, Start, Center, End, Stretch };
enum class Overflow { Visible, Hidden, Scroll, Auto };
enum class ScrollbarWidth { Auto, Thin, NoneValue };
enum class ScrollbarGutter { Auto, Stable, StableBothEdges };
enum class PointerEvents { Default, Auto, PassThrough };
enum class CursorStyle {
    Auto,
    Default,
    Pointer,
    Progress,
    Wait,
    Crosshair,
    Text,
    VerticalText,
    Alias,
    Copy,
    Move,
    NoDrop,
    NotAllowed,
    Grab,
    Grabbing,
    ColumnResize,
    RowResize,
    EastWestResize,
    NorthSouthResize,
    NortheastSouthwestResize,
    NorthwestSoutheastResize,
    AllScroll,
    ZoomIn,
    ZoomOut,
    Help,
    ContextMenu,
    Cell
};
enum class TextAlign { Left, Center, Right, Start, End };
enum class TextOverflow { Clip, Ellipsis, EllipsisCenter };
enum class TextWrap { Wrap, NoWrap };
enum class TextDecoration { NoneValue, Underline, LineThrough };
enum class VerticalAlign { Top, Middle, Bottom };
enum class FontFamily { Sans };

enum class InheritedStyleProperty : uint16_t {
    NotInherited = 0,
    FontFamily = 1 << 0,
    FontSize = 1 << 1,
    FontWeight = 1 << 2,
    FontStyle = 1 << 3,
    TextDecoration = 1 << 4,
    LineHeight = 1 << 5,
    Color = 1 << 6,
    TextAlign = 1 << 7,
    Cursor = 1 << 8,
    LetterSpacing = 1 << 9,
    WordSpacing = 1 << 10,
    TextWrap = 1 << 11,
    Visibility = 1 << 12,
    ScrollbarColor = 1 << 13,
    AccentColor = 1 << 14,
    ColorScheme = 1 << 15
};

using InheritedStyleProperties = uint16_t;

struct Style {
    AppearanceMode appearance = AppearanceMode::Auto;
    ColorScheme colorScheme = ColorScheme::Auto;
    BoxSizing boxSizing = BoxSizing::ContentBox;
    DisplayMode display = DisplayMode::Inline;
    bool displaySet = false;
    FlexDirection flexDirection = FlexDirection::Row;
    PositionMode position = PositionMode::Static;
    std::optional<GridArea> gridArea;
    Translate translate;
    Visibility visibility = Visibility::Visible;
    Color backgroundColor = Color(0.f, 0.f, 0.f, 0.f);
    std::optional<LightDarkColor> backgroundColorLightDark;
    bool backgroundColorCurrent = false;
    Color borderColor = Color(0.f, 0.f, 0.f, 1.f);
    std::optional<LightDarkColor> borderColorLightDark;
    bool borderColorCurrent = false;
    BorderStyle borderStyle = BorderStyle::Solid;
    Color color = Color(0.f, 0.f, 0.f, 1.f);
    std::optional<LightDarkColor> colorLightDark;
    AccentColor accentColor;
    Color iconStrokeColor = Color(0.f, 0.f, 0.f, 1.f);
    std::optional<LightDarkColor> iconStrokeColorLightDark;
    std::optional<Gradient> backgroundGradient;
    std::optional<Gradient> borderGradient;
    std::vector<BoxShadow> shadows;
    std::vector<Effect> effects;
    Outline outline;
    BorderRadii borderRadius;
    EdgeInsets borderWidth;
    bool borderWidthSet = false;
    bool borderColorSet = false;
    std::optional<Length> svgStrokeWidth;
    StrokeCap svgStrokeCap = StrokeCap::Butt;
    bool svgStrokeCapSet = false;
    float fontSize = 13.f;
    std::optional<Length> lineHeight;
    Length letterSpacing;
    Length wordSpacing;
    float opacity = 1.f;
    Dimension width;
    Dimension height;
    std::optional<Length> minWidth;
    std::optional<Length> minHeight;
    std::optional<Length> left;
    std::optional<Length> right;
    std::optional<Length> top;
    std::optional<Length> bottom;
    MarginInsets margin;
    EdgeInsets padding;
    GapValue gap;
    float flexGrow = 0.f;
    float flexShrink = 1.f;
    Dimension flexBasis;
    int order = 0;
    FontFamily fontFamily = FontFamily::Sans;
    U16 fontWeight = 400;
    bool fontItalic = false;
    TextDecoration textDecoration = TextDecoration::NoneValue;
    std::optional<std::string> content;
    TextAlign textAlign = TextAlign::Start;
    TextOverflow textOverflow = TextOverflow::Clip;
    TextWrap textWrap = TextWrap::Wrap;
    VerticalAlign verticalAlign = VerticalAlign::Top;
    bool verticalAlignSet = false;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    bool flexDirectionSet = false;
    JustifyContent justifyContent = JustifyContent::Start;
    bool justifyContentSet = false;
    JustifySelf justifySelf = JustifySelf::Auto;
    AlignItems alignItems = AlignItems::Normal;
    bool alignContentBlockCenter = false;
    AlignSelf alignSelf = AlignSelf::Auto;
    std::optional<float> aspectRatio;
    Overflow overflowX = Overflow::Visible;
    Overflow overflowY = Overflow::Visible;
    ScrollbarMode scrollbarMode = ScrollbarMode::Classic;
    bool scrollbarModeSet = false;
    ScrollbarWidth scrollbarWidth = ScrollbarWidth::Auto;
    ScrollbarGutter scrollbarGutter = ScrollbarGutter::Auto;
    ScrollbarColors scrollbarColor;
    PointerEvents pointerEvents = PointerEvents::Default;
    CursorStyle cursor = CursorStyle::Auto;
    InheritedStyleProperties specifiedInheritedProperties = 0;
    std::vector<std::string_view> explicitlyInheritedProperties;
};

void resolveLightDarkColors(Style& style);
void resolveCurrentColors(Style& style);
void normalizeOverflow(Style& style);
void inheritStyle(Style& style, const Style& parent);
void applyOpacity(Style& style, float inheritedOpacity);
} // namespace radia::ui
