/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <map>
#include <optional>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>
#include "types.h"

namespace radia::ui {
struct CustomPropertyValue {
    std::string source;

    friend bool operator==(const CustomPropertyValue&, const CustomPropertyValue&) = default;
};

using CustomPropertyMap = std::map<std::string, CustomPropertyValue>;

struct Length {
    float pixels = 0.f;
    float percent = 0.f;

    float resolve(float reference) const { return pixels + percent * reference; }
    bool isPercentage() const { return percent != 0.f; }
};

struct Percentage {
    float value = 0.f;
};

struct LengthPercentage {
    std::variant<Length, Percentage> value;
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

enum class DimensionKeyword { Content, MinContent, MaxContent, FitContent };

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

    static Dimension fromKeyword(DimensionKeyword keyword) {
        Dimension result;
        result.mKeyword = keyword;
        return result;
    }

    bool isAuto() const { return !mLength.has_value() && !mKeyword.has_value(); }
    bool isIntrinsic() const { return mKeyword.has_value(); }
    DimensionKeyword intrinsicKeyword() const { return mKeyword.value(); }
    float pixels() const { return mLength.value().pixels; }
    bool isPercentage() const { return mLength && mLength->isPercentage(); }
    float resolve(float fallback, float reference = 0.f) const { return mLength ? mLength->resolve(reference) : fallback; }

private:
    std::optional<Length> mLength;
    std::optional<DimensionKeyword> mKeyword;
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

    static GapValue fromPixels(float pixels) {
        GapValue result;
        result.mLength = Length{pixels};
        return result;
    }

    static GapValue fromLength(Length length) {
        GapValue result;
        result.mLength = length;
        return result;
    }

    float fixedPixels() const { return mLength.pixels; }

private:
    Length mLength;
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
    bool cornerDirection = false;
    float angleDegrees = 180.f;
    Vec2 center = {.5f, .5f};
    RadialGradientShape radialShape = RadialGradientShape::Ellipse;
    std::vector<GradientStop> stops;
};

enum class BackgroundRepeat { Repeat, NoRepeat, RepeatX, RepeatY };
enum class BackgroundBox { BorderBox, PaddingBox, ContentBox };
enum class BackgroundAttachment { Scroll, Fixed, Local };
enum class BackgroundSizeMode { Auto, Cover, Contain, Explicit };

struct BackgroundPosition {
    Length x{0.f, 0.f};
    Length y{0.f, 1.f};
};

struct BackgroundSize {
    BackgroundSizeMode mode = BackgroundSizeMode::Auto;
    std::optional<Length> width;
    std::optional<Length> height;
};

struct BackgroundLayer {
    std::string resource;
    std::optional<Gradient> gradient;
    BackgroundPosition position;
    BackgroundSize size;
    BackgroundRepeat repeat = BackgroundRepeat::Repeat;
    BackgroundBox origin = BackgroundBox::PaddingBox;
    BackgroundBox clip = BackgroundBox::BorderBox;
    BackgroundAttachment attachment = BackgroundAttachment::Scroll;
};

enum class MaskMode { MatchSource, Alpha, Luminance };
enum class MaskComposite { Add, Subtract, Intersect, Exclude };
enum class MaskType { Luminance, Alpha };

struct MaskLayer {
    MaskLayer() { image.origin = BackgroundBox::BorderBox; }

    BackgroundLayer image;
    MaskMode mode = MaskMode::MatchSource;
    MaskComposite composite = MaskComposite::Add;
    MaskType type = MaskType::Alpha;
};

struct BoxShadow {
    float horizontal = 0.f;
    float vertical = 0.f;
    float blur = 0.f;
    float spread = 0.f;
    Color color;
    bool currentColor = false;
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

struct BlurFilter {
    float stdDeviation = 0.f;
};

struct BlurStop {
    float stdDeviation = 0.f;
    float position = 0.f;
};

struct LinearBlurFilter {
    float angleDegrees = 180.f;
    std::vector<BlurStop> stops{{0.f, 0.f}, {0.f, 1.f}};
};

using FilterOperation = std::variant<BlurFilter, LinearBlurFilter>;
using FilterOperations = std::vector<FilterOperation>;

struct GridArea {
    int row = 1;
    int column = 1;
};

struct Translate {
    float x = 0.f;
    float y = 0.f;
};

enum class AppearanceMode { Auto, Base, NoneValue };
enum class ColorScheme { Light, Dark };

struct ColorSchemeContext {
    std::optional<ColorScheme> page;
    std::optional<ColorScheme> preference;
    bool preferenceOverriding = false;
    ColorScheme defaultScheme = ColorScheme::Dark;

    friend bool operator==(const ColorSchemeContext&, const ColorSchemeContext&) = default;
};

struct ColorSchemeValue {
    bool normal = true;
    bool only = false;
    std::vector<ColorScheme> schemes;
    std::vector<std::string> customIdentifiers;

    ColorScheme used(const ColorSchemeContext& context) const {
        if (normal) return context.page.value_or(context.defaultScheme);
        if (context.preference) {
            if (std::find(schemes.begin(), schemes.end(), *context.preference) != schemes.end()) return *context.preference;
            if (context.preferenceOverriding && !only && !schemes.empty()) return *context.preference;
        }
        return schemes.empty() ? context.defaultScheme : schemes.front();
    }

    friend bool operator==(const ColorSchemeValue&, const ColorSchemeValue&) = default;
};
enum class BoxSizing { ContentBox, BorderBox };
enum class DisplayMode { Inline, InlineBlock, Block, Flex, InlineFlex, Grid, InlineGrid, NoneValue };

inline constexpr bool isFlexDisplay(DisplayMode display) noexcept {
    return display == DisplayMode::Flex || display == DisplayMode::InlineFlex;
}

inline constexpr bool isOrderModifiedContainer(DisplayMode display) noexcept {
    return isFlexDisplay(display) || display == DisplayMode::Grid || display == DisplayMode::InlineGrid;
}
enum class BorderStyle { NoneValue, Solid, Outset, Inset };
enum class FlexDirection { Row, RowReverse, Column, ColumnReverse };
enum class PositionMode { Static, Relative, Absolute, Fixed, Sticky };
enum class OverflowAlignment : uint8_t { Default, Unsafe, Safe };
enum class ItemPosition : uint8_t {
    Auto,
    Normal,
    Stretch,
    Baseline,
    LastBaseline,
    Center,
    Start,
    End,
    SelfStart,
    SelfEnd,
    FlexStart,
    FlexEnd,
    Left,
    Right,
    AnchorCenter
};
enum class ContentPosition : uint8_t { Normal, Baseline, LastBaseline, Center, Start, End, FlexStart, FlexEnd, Left, Right };
enum class ContentDistribution : uint8_t { Default, SpaceBetween, SpaceAround, SpaceEvenly, Stretch };

struct SelfAlignmentData {
    ItemPosition position;
    OverflowAlignment overflow;
};

struct ContentAlignmentData {
    ContentPosition position;
    ContentDistribution distribution;
    OverflowAlignment overflow;
};
enum class FlexWrap { Nowrap, Wrap, WrapReverse };

inline constexpr bool isFlexWrapMultiLine(FlexWrap wrap) noexcept {
    return wrap != FlexWrap::Nowrap;
}

inline constexpr bool isFlexWrapReverse(FlexWrap wrap) noexcept {
    return wrap == FlexWrap::WrapReverse;
}

inline constexpr bool isRowFlexDirection(FlexDirection direction) noexcept {
    return direction == FlexDirection::Row || direction == FlexDirection::RowReverse;
}

inline constexpr bool isReverseFlexDirection(FlexDirection direction) noexcept {
    return direction == FlexDirection::RowReverse || direction == FlexDirection::ColumnReverse;
}
enum class Overflow { Visible, Hidden, Scroll, Auto };
enum class ScrollbarWidth { Auto, Thin, NoneValue };
enum class ScrollbarGutter { Auto, Stable, StableBothEdges };
enum class PointerEvents { Auto, NoneValue };
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

struct CursorImage {
    std::string resource;
    std::optional<float> hotspotX;
    std::optional<float> hotspotY;

    bool operator==(const CursorImage&) const = default;
};

struct CursorValue {
    CursorStyle style = CursorStyle::Auto;
    std::vector<CursorImage> images;

    bool operator==(const CursorValue&) const = default;
};
enum class TextAlign { Left, Center, Right, Start, End, Justify, MatchParent, JustifyAll };
enum class TextOverflow { Clip, Ellipsis, EllipsisCenter };
enum class TextWrap { Wrap, NoWrap };
enum class TextWrapStyle { Auto, Balance, Stable, Pretty, AvoidShortLastLine };
enum class TextDecoration : uint8_t { NoneValue = 0, Underline = 1 << 0, LineThrough = 1 << 1 };

inline constexpr bool hasTextDecoration(TextDecoration value, TextDecoration flag) {
    return (static_cast<uint8_t>(value) & static_cast<uint8_t>(flag)) != 0;
}
enum class VerticalAlign { Baseline, Sub, Super, TextTop, TextBottom, Top, Middle, Bottom, Length, Percentage };
enum class FontFamily { SansSerif, Monospace };

struct RelativeFontWeight {
    bool lighter = false;
};

using FontWeightValue = std::variant<float, RelativeFontWeight>;

struct LineHeight {
    enum class Kind { Normal, Number, Length };

    Kind kind = Kind::Normal;
    float value = 0.f;
};

struct FlexWrapValue {
    FlexWrap mode = FlexWrap::Nowrap;
    bool balance = false;
};

struct VerticalAlignValue {
    VerticalAlign value = VerticalAlign::Top;
    Length offset;
};

enum class InheritedStyleProperty : uint32_t {
    NotInherited = 0,
    FontFamily = 1 << 0,
    FontSize = 1 << 1,
    FontWeight = 1 << 2,
    FontStyle = 1 << 3,
    TextDecoration = 1 << 4,
    LineHeight = 1 << 5,
    Color = 1 << 6,
    TextAlign = 1 << 7,
    CursorPresentation = 1 << 8,
    LetterSpacing = 1 << 9,
    WordSpacing = 1 << 10,
    TextWrapMode = 1 << 11,
    TextWrapStyle = 1 << 12,
    Visibility = 1 << 13,
    ScrollbarColor = 1 << 14,
    AccentColor = 1 << 15,
    ColorScheme = 1 << 16
};

using InheritedStyleProperties = uint32_t;

struct ComputedStyle {
    AppearanceMode appearance = AppearanceMode::NoneValue;
    ColorSchemeValue colorScheme;
    ColorScheme usedColorScheme = ColorScheme::Dark;
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
    BorderStyle borderStyle = BorderStyle::NoneValue;
    Color color = Color(0.f, 0.f, 0.f, 1.f);
    std::optional<LightDarkColor> colorLightDark;
    AccentColor accentColor;
    Color strokeColor = Color(0.f, 0.f, 0.f, 1.f);
    std::optional<LightDarkColor> strokeColorLightDark;
    bool strokeColorCurrent = false;
    std::optional<Gradient> strokeGradient;
    std::optional<Gradient> backgroundGradient;
    std::vector<BackgroundLayer> backgroundLayers;
    std::vector<MaskLayer> maskLayers;
    std::optional<Gradient> borderGradient;
    std::vector<BoxShadow> shadows;
    FilterOperations filter;
    FilterOperations backdropFilter;
    Outline outline;
    BorderRadii borderRadius;
    EdgeInsets borderWidth;
    bool borderWidthSet = false;
    bool borderColorSet = false;
    std::optional<Length> svgStrokeWidth;
    StrokeCap svgStrokeCap = StrokeCap::Butt;
    bool svgStrokeCapSet = false;
    float fontSize = 13.f;
    LineHeight lineHeight;
    std::optional<float> lineHeightPercentage;
    Length letterSpacing;
    Length wordSpacing;
    float opacity = 1.f;
    Dimension width;
    Dimension height;
    std::optional<Dimension> minWidth;
    std::optional<Dimension> minHeight;
    std::optional<Dimension> maxWidth;
    std::optional<Dimension> maxHeight;
    std::optional<Length> left;
    std::optional<Length> right;
    std::optional<Length> top;
    std::optional<Length> bottom;
    MarginInsets margin;
    EdgeInsets padding;
    GapValue rowGap;
    GapValue columnGap;
    float flexGrow = 0.f;
    float flexShrink = 1.f;
    Dimension flexBasis;
    int order = 0;
    FontFamily fontFamily = FontFamily::SansSerif;
    U16 fontWeight = 400;
    std::optional<RelativeFontWeight> fontWeightAdjustment;
    bool fontItalic = false;
    TextDecoration textDecoration = TextDecoration::NoneValue;
    TextDecoration textDecorationPropagation = TextDecoration::NoneValue;
    std::optional<std::string> content;
    TextAlign textAlign = TextAlign::Start;
    TextOverflow textOverflow = TextOverflow::Clip;
    TextWrap textWrap = TextWrap::Wrap;
    TextWrapStyle textWrapStyle = TextWrapStyle::Auto;
    VerticalAlign verticalAlign = VerticalAlign::Top;
    Length verticalAlignOffset;
    bool verticalAlignSet = false;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    bool flexDirectionSet = false;
    ContentAlignmentData justifyContent{ContentPosition::Normal, ContentDistribution::Default, OverflowAlignment::Default};
    bool justifyContentSet = false;
    SelfAlignmentData justifySelf{ItemPosition::Auto, OverflowAlignment::Default};
    SelfAlignmentData justifyItems{ItemPosition::Normal, OverflowAlignment::Default};
    SelfAlignmentData alignItems{ItemPosition::Normal, OverflowAlignment::Default};
    ContentAlignmentData alignContent{ContentPosition::Normal, ContentDistribution::Default, OverflowAlignment::Default};
    FlexWrap flexWrap = FlexWrap::Nowrap;
    bool flexWrapBalance = false;
    bool alignContentBlockCenter = false;
    SelfAlignmentData alignSelf{ItemPosition::Auto, OverflowAlignment::Default};
    std::optional<float> aspectRatio;
    Overflow overflowX = Overflow::Visible;
    Overflow overflowY = Overflow::Visible;
    ScrollbarWidth scrollbarWidth = ScrollbarWidth::Auto;
    ScrollbarGutter scrollbarGutter = ScrollbarGutter::Auto;
    ScrollbarColors scrollbarColor;
    PointerEvents pointerEvents = PointerEvents::Auto;
    bool pointerEventsSpecified = false;
    CursorStyle cursor = CursorStyle::Auto;
    std::vector<CursorImage> cursorImages;
    InheritedStyleProperties specifiedInheritedProperties = 0;
    std::vector<std::string_view> explicitlyInheritedProperties;
    CustomPropertyMap customProperties;
};

void resolveLightDarkColors(ComputedStyle& style, const ColorSchemeContext& context = {});
void resolveCurrentColors(ComputedStyle& style);
void normalizeOverflow(ComputedStyle& style);
void inheritStyle(ComputedStyle& style, const ComputedStyle& parent);
void resolveRelativeFontWeight(ComputedStyle& style, U16 inheritedWeight = 400);
void resolvePercentageLineHeight(ComputedStyle& style);
void applyOpacity(ComputedStyle& style, float inheritedOpacity);
} // namespace radia::ui
