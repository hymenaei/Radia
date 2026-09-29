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
#include "CSSKeywords.h"
#include "CSSValue.h"
#include "Color.h"
#include "LayoutGeometry.h"
#include "Ref.h"

namespace Core::Style {
struct Nonnegative {};
struct UnitInterval {};
enum class Visibility : std::uint8_t {
    Visible,
    Hidden,
    Collapse
};
enum class StrokeCap {
    Butt,
    Round,
    Square
};

struct CustomPropertyValue {
    std::string source;

    friend bool operator==(const CustomPropertyValue&, const CustomPropertyValue&) = default;
};

using CustomPropertyMap = std::map<std::string, CustomPropertyValue>;

enum class ColorSchemeMode : std::uint8_t {
    Light,
    Dark
};
Core::Color systemColorValue(CSS::KeywordName keyword, ColorSchemeMode scheme);

template<typename Constraint, typename T> struct Number {
    T value = 0;

    constexpr Number() = default;
    constexpr explicit Number(T value)
        : value(value) {}
    constexpr Number(CSS::Number value)
        : value(value.value) {}

    friend bool operator==(const Number&, const Number&) = default;
};

template<typename Constraint, typename T> struct LengthValue {
    T pixels = 0;
    T percent = 0;

    T resolve(T reference) const { return pixels + percent * reference; }
    bool isPercentage() const { return percent != 0.f; }

    friend bool operator==(const LengthValue&, const LengthValue&) = default;
};

inline std::optional<float> absolutePixels(CSS::Length length) {
    switch (length.unit) {
    case CSS::LengthUnit::Px:
        return length.value;
    case CSS::LengthUnit::In:
        return length.value * 96.f;
    case CSS::LengthUnit::Cm:
        return length.value * (96.f / 2.54f);
    case CSS::LengthUnit::Mm:
        return length.value * (96.f / 25.4f);
    case CSS::LengthUnit::Q:
        return length.value * (96.f / 101.6f);
    case CSS::LengthUnit::Pt:
        return length.value * (96.f / 72.f);
    case CSS::LengthUnit::Pc:
        return length.value * 16.f;
    default:
        return std::nullopt;
    }
}

using LengthPercentage = LengthValue<Nonnegative, float>;
using Length = LengthPercentage;
using InsetEdge = std::optional<Length>;

struct LetterSpacingTag {};
using LetterSpacing = LengthValue<LetterSpacingTag, float>;
struct WordSpacingTag {};
using WordSpacing = LengthValue<WordSpacingTag, float>;

struct CornerRadius {
    LengthPercentage horizontal;
    LengthPercentage vertical;

    CornerRadius() = default;
    CornerRadius(CSS::Length value)
        : horizontal {value.value}
        , vertical {value.value} {}
    CornerRadius(LengthPercentage horizontal, LengthPercentage vertical)
        : horizontal(horizontal)
        , vertical(vertical) {}

    static CornerRadius uniform(LengthPercentage radius) { return {radius, radius}; }

    friend bool operator==(const CornerRadius&, const CornerRadius&) = default;
};

struct BorderRadius {
    CornerRadius topLeft;
    CornerRadius topRight;
    CornerRadius bottomRight;
    CornerRadius bottomLeft;

    BorderRadius() = default;
    BorderRadius(CornerRadius topLeft, CornerRadius topRight, CornerRadius bottomRight, CornerRadius bottomLeft);
    static BorderRadius uniform(LengthPercentage radius) {
        const CornerRadius corner = CornerRadius::uniform(radius);
        return {corner, corner, corner, corner};
    }

    friend bool operator==(const BorderRadius&, const BorderRadius&) = default;
};

class Color {
public:
    Color() = default;
    Color(Core::Color value)
        : mValue(value) {}
    Color(CSS::Keyword::CurrentColor)
        : mValue(CSS::Keyword::CurrentColor::value) {}
    Color(CSS::Keyword::Transparent)
        : mValue(Core::Color {0.f, 0.f, 0.f, 0.f}) {}

    static Color fromColor(Core::Color value) { return Color(value); }
    static Color current();
    static Color fromKeyword(CSS::KeywordName keyword);
    static std::optional<Color> fromCSS(const CSS::Color&, ColorSchemeMode);

    bool isResolved() const { return std::holds_alternative<Core::Color>(mValue); }
    bool isCurrentColor() const;
    bool isSystemColor() const;
    const Core::Color& resolvedColor() const { return std::get<Core::Color>(mValue); }
    Core::Color& resolvedColor() { return std::get<Core::Color>(mValue); }
    void resolve(const Core::Color& currentColor, ColorSchemeMode scheme);

    friend bool operator==(const Color&, const Color&) = default;

private:
    explicit Color(CSS::KeywordName keyword)
        : mValue(keyword) {}

    std::variant<Core::Color, CSS::KeywordName> mValue {Core::Color {}};
};

struct ScrollbarColor {
    bool automatic = true;
    Color thumb;
    Color track;

    friend bool operator==(const ScrollbarColor&, const ScrollbarColor&) = default;
};

template<typename ValueType, typename KeywordType> struct ValueOrKeyword {
    using Value = ValueType;
    using Keyword = KeywordType;

    std::variant<Keyword, Value> mValue;

    ValueOrKeyword(Keyword value)
        : mValue(value) {}
    ValueOrKeyword(Value value)
        : mValue(std::move(value)) {}

    bool isKeyword() const { return std::holds_alternative<Keyword>(mValue); }
    const Value& value() const { return std::get<Value>(mValue); }
    Value& value() { return std::get<Value>(mValue); }

    friend bool operator==(const ValueOrKeyword&, const ValueOrKeyword&) = default;
};

using AccentColor = ValueOrKeyword<Color, CSS::Keyword::Auto>;

enum class DimensionKeyword {
    Content,
    MinContent,
    MaxContent,
    FitContent
};

class Dimension {
public:
    constexpr Dimension() = default;

    static Dimension fromPixels(float pixels) {
        Dimension result;
        result.mLength = Length {pixels};
        return result;
    }

    static Dimension fromLength(Length length) {
        Dimension result;
        result.mLength = length;
        result.mPercentage = length.percent != 0.f;
        return result;
    }

    static Dimension fromPercentage(float percent) {
        Dimension result;
        result.mLength = Length {0.f, percent};
        result.mPercentage = true;
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
    bool isPercentage() const { return mLength && mPercentage; }
    float resolve(float fallback, float reference = 0.f) const { return mLength ? mLength->resolve(reference) : fallback; }

    friend bool operator==(const Dimension&, const Dimension&) = default;

private:
    std::optional<Length> mLength;
    std::optional<DimensionKeyword> mKeyword;
    bool mPercentage = false;
};

using PreferredSize = Dimension;
using FlexBasis = Dimension;
using OptionalDimension = std::optional<Dimension>;
using MaximumSize = OptionalDimension;
using MinimumSize = OptionalDimension;
struct PaddingEdgeTag {};
using PaddingEdge = LengthValue<PaddingEdgeTag, float>;

class MarginEdge {
public:
    MarginEdge() = default;

    static MarginEdge automatic() {
        MarginEdge result;
        result.mAutomatic = true;
        return result;
    }

    static MarginEdge fromPixels(float pixels) { return MarginEdge(Length {pixels}); }
    static MarginEdge fromLength(Length length) { return MarginEdge(length); }

    bool isAuto() const { return mAutomatic; }
    float fixedPixels() const { return mAutomatic ? 0.f : mLength.pixels; }
    float resolve(float reference) const { return mAutomatic ? 0.f : mLength.resolve(reference); }

    friend bool operator==(const MarginEdge&, const MarginEdge&) = default;

private:
    explicit MarginEdge(Length length)
        : mLength(length) {}

    bool mAutomatic = false;
    Length mLength;
};

class GapGutter {
public:
    GapGutter() = default;

    static GapGutter fromPixels(float pixels) {
        GapGutter result;
        result.mLength = Length {pixels};
        return result;
    }

    float fixedPixels() const { return mLength.pixels; }

    friend bool operator==(const GapGutter&, const GapGutter&) = default;

private:
    Length mLength;
};

struct GradientStop {
    Color color;
    float position = 0.f;

    friend bool operator==(const GradientStop&, const GradientStop&) = default;
};

enum class GradientKind {
    Linear,
    Radial,
    Conic
};

enum class RadialGradientShape {
    Ellipse,
    Circle
};

struct Gradient {
    GradientKind kind = GradientKind::Linear;
    bool repeating = false;
    bool cornerDirection = false;
    float angleDegrees = 180.f;
    Layout::Vec2 center = {.5f, .5f};
    RadialGradientShape radialShape = RadialGradientShape::Ellipse;
    std::vector<GradientStop> stops;

    friend bool operator==(const Gradient&, const Gradient&) = default;
};

struct Image {
    using Value = std::variant<std::monostate, std::string, Gradient>;

    Value value;

    friend bool operator==(const Image&, const Image&) = default;

    std::string* resource() { return std::get_if<std::string>(&value); }
    const std::string* resource() const { return std::get_if<std::string>(&value); }
    Gradient* gradient() { return std::get_if<Gradient>(&value); }
    const Gradient* gradient() const { return std::get_if<Gradient>(&value); }
};

struct BorderImageSliceValue {
    float value;
    bool percentage;

    friend bool operator==(const BorderImageSliceValue&, const BorderImageSliceValue&) = default;
};

struct BorderImageSlice {
    Layout::RectEdges<BorderImageSliceValue> edges;
    bool fill;

    BorderImageSlice() = default;
    BorderImageSlice(CSS::Number value)
        : edges {{value.value, false}, {value.value, false}, {value.value, false}, {value.value, false}}
        , fill(false) {}
    BorderImageSlice(CSS::Percentage value)
        : edges {{value.value, true}, {value.value, true}, {value.value, true}, {value.value, true}}
        , fill(false) {}

    friend bool operator==(const BorderImageSlice&, const BorderImageSlice&) = default;
};

enum class BorderImageValueUnit : std::uint8_t {
    Number,
    Length,
    Percentage,
    Auto
};

struct BorderImageWidthValue {
    float value;
    BorderImageValueUnit unit;

    friend bool operator==(const BorderImageWidthValue&, const BorderImageWidthValue&) = default;
};

struct BorderImageWidth {
    Layout::RectEdges<BorderImageWidthValue> edges;

    constexpr BorderImageWidth() = default;
    constexpr BorderImageWidth(CSS::Number value)
        : edges {{value.value, BorderImageValueUnit::Number}, {value.value, BorderImageValueUnit::Number},
              {value.value, BorderImageValueUnit::Number}, {value.value, BorderImageValueUnit::Number}} {}

    friend bool operator==(const BorderImageWidth&, const BorderImageWidth&) = default;
};

struct BorderImageOutsetValue {
    float value;
    bool multiplier;

    friend bool operator==(const BorderImageOutsetValue&, const BorderImageOutsetValue&) = default;
};

struct BorderImageOutset {
    Layout::RectEdges<BorderImageOutsetValue> edges;

    constexpr BorderImageOutset() = default;
    constexpr BorderImageOutset(CSS::Number value)
        : edges {{value.value, true}, {value.value, true}, {value.value, true}, {value.value, true}} {}

    friend bool operator==(const BorderImageOutset&, const BorderImageOutset&) = default;
};

enum class BorderImageRepeatMode : std::uint8_t {
    Stretch,
    Repeat,
    Round,
    Space
};

struct BorderImageRepeat {
    BorderImageRepeatMode horizontal;
    BorderImageRepeatMode vertical;

    friend bool operator==(const BorderImageRepeat&, const BorderImageRepeat&) = default;
};

struct BorderImage {
    Image source;
    BorderImageSlice slice;
    BorderImageWidth width;
    BorderImageOutset outset;
    BorderImageRepeat repeat;
};

enum class BackgroundRepeat {
    Repeat,
    NoRepeat,
    RepeatX,
    RepeatY
};
enum class BackgroundBox {
    BorderBox,
    PaddingBox,
    ContentBox
};
enum class BackgroundAttachment {
    Scroll,
    Fixed,
    Local
};
enum class BackgroundSizeType {
    Auto,
    Cover,
    Contain,
    Explicit
};

struct BackgroundPosition {
    Length x {0.f, 0.f};
    Length y {0.f, 1.f};
};

struct BackgroundSize {
    BackgroundSizeType mode = BackgroundSizeType::Auto;
    std::optional<Length> width;
    std::optional<Length> height;
};

struct BackgroundLayer {
    Image image;
    BackgroundPosition position;
    BackgroundSize size;
    BackgroundRepeat repeat = BackgroundRepeat::Repeat;
    BackgroundBox origin = BackgroundBox::PaddingBox;
    BackgroundBox clip = BackgroundBox::BorderBox;
    BackgroundAttachment attachment = BackgroundAttachment::Scroll;
};

enum class MaskMode {
    MatchSource,
    Alpha,
    Luminance
};
enum class MaskComposite {
    Add,
    Subtract,
    Intersect,
    Exclude
};
enum class MaskType {
    Luminance,
    Alpha
};

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
    Color color = Color::current();
    bool inset = false;

    friend bool operator==(const BoxShadow&, const BoxShadow&) = default;
};

using BoxShadows = std::vector<BoxShadow>;

enum class OutlineStyle {
    Solid,
    Dashed
};

struct Outline {
    float width = 0.f;
    Length offset;
    Color color;
    OutlineStyle style = OutlineStyle::Solid;
};
using OutlineOffset = Length;

struct BlurFilter {
    float stdDeviation = 0.f;

    friend bool operator==(const BlurFilter&, const BlurFilter&) = default;
};

struct BlurStop {
    float stdDeviation = 0.f;
    float position = 0.f;

    friend bool operator==(const BlurStop&, const BlurStop&) = default;
};

struct LinearBlurFilter {
    float angleDegrees = 180.f;
    std::vector<BlurStop> stops {{0.f, 0.f}, {0.f, 1.f}};

    friend bool operator==(const LinearBlurFilter&, const LinearBlurFilter&) = default;
};

using FilterOperation = std::variant<BlurFilter, LinearBlurFilter>;

struct Filter {
    std::vector<FilterOperation> operations;

    bool isNone() const { return operations.empty(); }

    friend bool operator==(const Filter&, const Filter&) = default;
};

struct GridArea {
    int row = 1;
    int column = 1;
};

struct Translate {
    Length x;
    Length y;
    Length z;
    bool isNone = true;

    friend bool operator==(const Translate&, const Translate&) = default;
};

enum class Appearance {
    Auto,
    Base,
    NoneValue
};

struct ColorSchemeContext {
    std::optional<ColorSchemeMode> page;
    std::optional<ColorSchemeMode> preference;
    bool preferenceOverriding = false;
    ColorSchemeMode defaultScheme = ColorSchemeMode::Dark;

    friend bool operator==(const ColorSchemeContext&, const ColorSchemeContext&) = default;
};

struct ColorScheme {
    bool normal = true;
    bool only = false;
    std::vector<ColorSchemeMode> schemes;
    std::vector<std::string> customIdentifiers;

    ColorSchemeMode used(const ColorSchemeContext& context) const {
        if (normal)
            return context.page.value_or(context.defaultScheme);
        if (context.preference) {
            if (std::find(schemes.begin(), schemes.end(), *context.preference) != schemes.end())
                return *context.preference;
            if (context.preferenceOverriding && !only && !schemes.empty())
                return *context.preference;
        }
        return schemes.empty() ? context.defaultScheme : schemes.front();
    }

    friend bool operator==(const ColorScheme&, const ColorScheme&) = default;
};
enum class BoxSizing {
    ContentBox,
    BorderBox
};
enum class Display {
    Inline,
    InlineBlock,
    Block,
    Flex,
    InlineFlex,
    Grid,
    InlineGrid,
    NoneValue
};

inline constexpr bool isFlexDisplay(Display display) noexcept { return display == Display::Flex || display == Display::InlineFlex; }

inline constexpr bool isOrderModifiedContainer(Display display) noexcept {
    return isFlexDisplay(display) || display == Display::Grid || display == Display::InlineGrid;
}
enum class BorderStyle {
    NoneValue,
    Solid,
    Outset,
    Inset
};
enum class FlexDirection {
    Row,
    RowReverse,
    Column,
    ColumnReverse
};
enum class Position {
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky
};
enum class OverflowAlignment : uint8_t {
    Default,
    Unsafe,
    Safe
};
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
enum class ContentPosition : uint8_t {
    Normal,
    Baseline,
    LastBaseline,
    Center,
    Start,
    End,
    FlexStart,
    FlexEnd,
    Left,
    Right
};
enum class ContentDistribution : uint8_t {
    Default,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
    Stretch
};

struct SelfAlignmentData {
    ItemPosition position;
    OverflowAlignment overflow;

    friend bool operator==(const SelfAlignmentData&, const SelfAlignmentData&) = default;
};

struct JustifyItems : SelfAlignmentData {
    constexpr JustifyItems(ItemPosition position = ItemPosition::Normal, OverflowAlignment overflow = OverflowAlignment::Default,
        bool legacy = false)
        : SelfAlignmentData {position, overflow}
        , legacy(legacy) {}

    bool legacy;

    friend bool operator==(const JustifyItems&, const JustifyItems&) = default;
};

struct ContentAlignmentData {
    ContentPosition position;
    ContentDistribution distribution;
    OverflowAlignment overflow;

    friend bool operator==(const ContentAlignmentData&, const ContentAlignmentData&) = default;
};
enum class FlexWrapMode : std::uint8_t {
    Nowrap,
    Wrap,
    WrapReverse
};

struct FlexWrap {
    FlexWrapMode mode = FlexWrapMode::Nowrap;
    bool balance = false;

    static constexpr FlexWrap fromRaw(unsigned raw) { return {static_cast<FlexWrapMode>(raw & 0x3u), (raw & 0x4u) != 0}; }

    constexpr unsigned toRaw() const { return static_cast<unsigned>(mode) | (balance ? 0x4u : 0u); }

    friend constexpr bool operator==(const FlexWrap&, const FlexWrap&) = default;
};

inline constexpr bool isFlexWrapMultiLine(FlexWrap wrap) noexcept { return wrap.mode != FlexWrapMode::Nowrap; }

inline constexpr bool isFlexWrapReverse(FlexWrap wrap) noexcept { return wrap.mode == FlexWrapMode::WrapReverse; }

inline constexpr bool isRowFlexDirection(FlexDirection direction) noexcept {
    return direction == FlexDirection::Row || direction == FlexDirection::RowReverse;
}

inline constexpr bool isReverseFlexDirection(FlexDirection direction) noexcept {
    return direction == FlexDirection::RowReverse || direction == FlexDirection::ColumnReverse;
}
enum class Overflow {
    Visible,
    Hidden,
    Scroll,
    Auto,
    Clip
};
enum class ScrollbarWidth {
    Auto,
    Thin,
    NoneValue
};
enum class ScrollbarGutter {
    Auto,
    Stable,
    StableBothEdges
};
enum class PointerEvents {
    Auto,
    NoneValue
};
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
enum class TextAlign {
    Left,
    Center,
    Right,
    Start,
    End,
    Justify,
    MatchParent,
    JustifyAll
};
enum class TextOverflow {
    Clip,
    Ellipsis,
    EllipsisCenter
};
enum class TextWrapMode {
    Wrap,
    NoWrap
};
enum class TextWrapStyle {
    Auto,
    Balance,
    Stable,
    Pretty,
    AvoidShortLastLine
};

enum class TextDecoration : uint8_t {
    NoneValue = 0,
    Underline = 1 << 0,
    LineThrough = 1 << 1
};

inline constexpr bool hasTextDecoration(TextDecoration value, TextDecoration flag) {
    return (static_cast<uint8_t>(value) & static_cast<uint8_t>(flag)) != 0;
}
enum class VerticalAlign {
    Baseline,
    Sub,
    Super,
    TextTop,
    TextBottom,
    Top,
    Middle,
    Bottom,
    Length,
    Percentage
};
enum class GenericFontFamily : std::uint8_t {
    Serif,
    SansSerif,
    SystemUI,
    Cursive,
    Fantasy,
    Math,
    Monospace,
    UISerif,
    UISansSerif,
    UIMonospace,
    UIRounded,
    Fangsong,
    Kai,
    KhmerMul,
    Nastaliq
};

using FontFamily = std::variant<std::string, GenericFontFamily>;
using FontFamilies = std::vector<FontFamily>;
enum class FontStyle {
    Normal,
    Italic,
    Oblique
};

struct VerticalAlignValue {
    VerticalAlign value = VerticalAlign::Baseline;
    Length offset;

    friend bool operator==(const VerticalAlignValue&, const VerticalAlignValue&) = default;
};

struct FontWidth {
    float percentage = 100.f;

    constexpr FontWidth() = default;
    constexpr FontWidth(float percentage)
        : percentage(percentage) {}
    constexpr operator float() const { return percentage; }

    friend bool operator==(const FontWidth&, const FontWidth&) = default;
};

struct FontWeight {
    float value = 400.f;

    constexpr FontWeight() = default;
    constexpr explicit FontWeight(float value)
        : value(value) {}

    friend bool operator==(const FontWeight&, const FontWeight&) = default;
};

struct FontSelectionRequest {
    FontWeight weight;
    FontWidth width;
    FontStyle style = FontStyle::Normal;

    friend bool operator==(const FontSelectionRequest&, const FontSelectionRequest&) = default;
};

struct LineHeight {
    using Number = Number<Nonnegative, float>;
    using Length = LengthValue<Nonnegative, float>;

    constexpr LineHeight(CSS::Keyword::Normal keyword)
        : mValue(keyword) {}
    constexpr LineHeight(Number number)
        : mValue(number) {}
    constexpr LineHeight(Length length)
        : mValue(length) {}

    std::variant<CSS::Keyword::Normal, Number, Length> mValue;

    friend bool operator==(const LineHeight&, const LineHeight&) = default;
};

using AlignContent = ContentAlignmentData;
using AlignItems = SelfAlignmentData;
using AlignSelf = SelfAlignmentData;
using JustifyContent = ContentAlignmentData;
using JustifySelf = SelfAlignmentData;

struct LineWidth {
    float pixels = 0.f;

    constexpr LineWidth() = default;
    constexpr explicit LineWidth(float pixels)
        : pixels(pixels) {}
    constexpr LineWidth(CSS::Keyword::Thin)
        : pixels(1.f) {}
    constexpr LineWidth(CSS::Keyword::Medium)
        : pixels(3.f) {}
    constexpr LineWidth(CSS::Keyword::Thick)
        : pixels(5.f) {}

    static constexpr std::optional<LineWidth> fromCSSKeyword(CSS::KeywordName keyword) {
        using enum CSS::KeywordName;
        switch (keyword) {
        case KeywordThin:
            return LineWidth {CSS::Keyword::Thin {}};
        case KeywordMedium:
            return LineWidth {CSS::Keyword::Medium {}};
        case KeywordThick:
            return LineWidth {CSS::Keyword::Thick {}};
        default:
            return std::nullopt;
        }
    }

    static std::optional<LineWidth> fromCSSValue(const CSS::Value& value) {
        if (const auto* length = std::get_if<CSS::Length>(&value)) {
            const auto pixels = absolutePixels(*length);
            if (!pixels)
                return std::nullopt;
            return LineWidth {*pixels};
        }
        if (const auto* keyword = std::get_if<CSS::Keyword>(&value))
            return fromCSSKeyword(keyword->keyword);
        return std::nullopt;
    }

    friend bool operator==(const LineWidth&, const LineWidth&) = default;
};

struct BorderData {
    Layout::RectEdges<LineWidth> widths;
    Layout::RectEdges<unsigned> styles;
    Layout::RectEdges<Color> colors;
    BorderRadius radii;
    BorderImage borderImage;

    BorderData();
};

struct NonInheritedFlags {
    bool internalAlignContentBlock;
    unsigned boxSizing : 1;
    unsigned display : 3;
    unsigned overflowX : 3;
    unsigned overflowY : 3;
    unsigned position : 3;
};

struct InheritedFlags {
    unsigned textAlign : 3;
    unsigned visibility : 2;
    unsigned pointerEvents : 1;
    unsigned textWrapMode : 1;
    unsigned textWrapStyle : 3;
};

using FlexGrow = Number<Nonnegative, float>;
using FlexShrink = Number<Nonnegative, float>;
using Opacity = Number<UnitInterval, float>;

struct Order {
    int value = 0;

    friend constexpr bool operator==(const Order&, const Order&) = default;
};

struct FlexibleBoxData : RefCounted<FlexibleBoxData> {
    static Ref<FlexibleBoxData> create() { return adoptRef(*new FlexibleBoxData); }
    Ref<FlexibleBoxData> copy() const;

    FlexGrow flexGrow;
    FlexShrink flexShrink;
    FlexBasis flexBasis;
    unsigned flexDirection : 2;
    unsigned flexWrap : 3;

private:
    FlexibleBoxData();
    FlexibleBoxData(const FlexibleBoxData&);
};

struct BoxData {
    PreferredSize width;
    PreferredSize height;
    MinimumSize minWidth;
    MinimumSize minHeight;
    MaximumSize maxWidth;
    MaximumSize maxHeight;
    VerticalAlignValue verticalAlign;

    BoxData();
};

struct NonInheritedMiscData : RefCounted<NonInheritedMiscData> {
    static Ref<NonInheritedMiscData> create() { return adoptRef(*new NonInheritedMiscData); }
    Ref<NonInheritedMiscData> copy() const;

    AlignContent alignContent;
    AlignItems alignItems;
    AlignSelf alignSelf;
    JustifyContent justifyContent;
    JustifyItems justifyItems;
    JustifySelf justifySelf;
    Ref<FlexibleBoxData> flexibleBox;
    Opacity opacity;
    Order order;
    GapGutter rowGap;
    GapGutter columnGap;
    Translate translate;
    BoxShadows shadows;
    unsigned textDecoration : 2;
    unsigned textOverflow : 2;
    unsigned appearance : 2;

private:
    NonInheritedMiscData();
    NonInheritedMiscData(const NonInheritedMiscData&);
};

struct NonInheritedRareData : RefCounted<NonInheritedRareData> {
    static Ref<NonInheritedRareData> create() { return adoptRef(*new NonInheritedRareData); }
    Ref<NonInheritedRareData> copy() const;

    Filter backdropFilter;
    ScrollbarGutter scrollbarGutter;
    unsigned scrollbarWidth : 2;

private:
    NonInheritedRareData();
    NonInheritedRareData(const NonInheritedRareData&);
};

struct FontDescription {
    static constexpr float kCssMediumSize = 13.f;

    FontDescription();

    float fontSize() const;
    void setFontSize(float);
    FontStyle fontStyle() const;
    void setFontStyle(FontStyle);
    FontWeight fontWeight() const;
    void setFontWeight(FontWeight);
    FontWidth fontWidth() const;
    void setFontWidth(FontWidth);

    bool operator==(const FontDescription&) const = default;

private:
    void setFontSize(CSS::Keyword::Medium);
    void setFontWeight(CSS::Keyword::Normal);
    void setFontWidth(CSS::Keyword::Normal);

    float mFontSize = 0.f;
    FontSelectionRequest mSelection;
};

struct FontCascadeDescription : FontDescription {
    FontCascadeDescription();

    const FontFamilies& families() const;
    void setFamilies(FontFamilies);

    bool operator==(const FontCascadeDescription&) const = default;

private:
    FontFamilies mFamilies;
};

struct FontData : RefCounted<FontData> {
    static Ref<FontData> create() { return adoptRef(*new FontData); }
    Ref<FontData> copy() const;

    FontCascadeDescription fontCascade;
    LetterSpacing letterSpacing;
    WordSpacing wordSpacing;

    bool operator==(const FontData&) const;

private:
    FontData();
    FontData(const FontData&);
};

struct InheritedData : RefCounted<InheritedData> {
    static Ref<InheritedData> create() { return adoptRef(*new InheritedData); }
    Ref<InheritedData> copy() const;

    Ref<FontData> fontData;
    Color color;
    LineHeight lineHeight;

private:
    friend struct ComputedStyle;
    InheritedData();
    InheritedData(const InheritedData&);
};

struct InheritedRareData : RefCounted<InheritedRareData> {
    static Ref<InheritedRareData> create() { return adoptRef(*new InheritedRareData); }
    Ref<InheritedRareData> copy() const;

    AccentColor accentColor;
    ColorScheme colorScheme;
    ScrollbarColor scrollbarColor;

private:
    friend struct ComputedStyle;
    InheritedRareData();
    InheritedRareData(const InheritedRareData&);
};

struct BackgroundData : RefCounted<BackgroundData> {
    static Ref<BackgroundData> create() { return adoptRef(*new BackgroundData); }
    Ref<BackgroundData> copy() const;

    Color backgroundColor;
    Outline outline;

private:
    BackgroundData();
    BackgroundData(const BackgroundData&);
};

struct SurroundData : RefCounted<SurroundData> {
    static Ref<SurroundData> create() { return adoptRef(*new SurroundData); }
    Ref<SurroundData> copy() const;

    BorderData border;
    Layout::RectEdges<MarginEdge> margin;
    Layout::RectEdges<PaddingEdge> padding;
    Layout::RectEdges<InsetEdge> inset;

private:
    SurroundData();
    SurroundData(const SurroundData&);
};

struct NonInheritedData : RefCounted<NonInheritedData> {
    static Ref<NonInheritedData> create() { return adoptRef(*new NonInheritedData); }
    Ref<NonInheritedData> copy() const;

    Ref<BackgroundData> backgroundData;
    Ref<SurroundData> surroundData;
    Ref<NonInheritedMiscData> miscData;
    Ref<NonInheritedRareData> rareData;
    BoxData boxData;

private:
    NonInheritedData();
    NonInheritedData(const NonInheritedData&);
};

struct ComputedStyle {
public:
    ComputedStyle();
    static const ComputedStyle& initialStyle();

#include "ComputedStylePropertiesInlines.h"

    const Outline& outline() const { return mNonInheritedData->backgroundData->outline; }
    Outline& outline() { return mNonInheritedData.access().backgroundData.access().outline; }

    ColorSchemeMode usedColorScheme = ColorSchemeMode::Dark;
    bool displaySet = false;
    std::optional<GridArea> gridArea;
    std::variant<Color, Image> stroke = Color::fromColor(Core::Color(0.f, 0.f, 0.f, 1.f));
    std::vector<BackgroundLayer> backgroundLayers;
    std::vector<MaskLayer> maskLayers;
    Filter filter;
    std::optional<Length> svgStrokeWidth;
    StrokeCap svgStrokeCap = StrokeCap::Butt;
    bool svgStrokeCapSet = false;
    TextDecoration textDecorationPropagation = TextDecoration::NoneValue;
    std::optional<std::string> content;
    Layout::Direction direction = Layout::Direction::LeftToRight;
    bool alignContentBlockCenter = false;
    std::optional<float> aspectRatio;
    bool pointerEventsSpecified = false;
    CursorStyle cursor = CursorStyle::Auto;
    std::vector<CursorImage> cursorImages;
    std::vector<std::string> specifiedProperties;
    std::vector<std::string> explicitlyInheritedProperties;
    CustomPropertyMap customProperties;

private:
    InheritedFlags mInheritedFlags;
    NonInheritedFlags mNonInheritedFlags;
    Ref<InheritedData> mInheritedData;
    Ref<InheritedRareData> mInheritedRareData;
    Ref<NonInheritedData> mNonInheritedData;
};

struct BuilderContext {
    const ComputedStyle& style;
    const ComputedStyle* parentStyle;
    const ComputedStyle& rootStyle;

    BuilderContext(const ComputedStyle& style, const ComputedStyle* parentStyle = nullptr,
        const ComputedStyle& rootStyle = ComputedStyle::initialStyle());
};

struct BuilderState {
    ComputedStyle& style;
    const ComputedStyle* parentStyle;
    BuilderContext context;

    BuilderState(ComputedStyle& style, const ComputedStyle* parentStyle = nullptr,
        const ComputedStyle& rootStyle = ComputedStyle::initialStyle());
};

void resolveStyleColors(ComputedStyle& style, const Core::Color& inheritedColor);
void normalizeOverflow(ComputedStyle& style);
void inheritStyle(ComputedStyle& style, const ComputedStyle& parent);
void applyOpacity(ComputedStyle& style, float inheritedOpacity);
} // namespace Core::Style
