/**
 * @file style.h
 * @brief Defines typed RSL style values, layout properties, states, and inheritance metadata.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
 */

#ifndef RD_STYLE_H
#define RD_STYLE_H

#include <cstddef>
#include <cstdint>
#include <optional>
#include <vector>
#include "types.h"

namespace radia::ui {
struct Length {
    float pixels = 0.f;
    float percent = 0.f;

    float resolve(float reference) const { return pixels + percent * reference; }
    bool isPercentage() const { return percent != 0.f; }
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
};

enum class OutlineStyle { Solid, Dashed };

struct Outline {
    float width = 0.f;
    float offset = 0.f;
    Color color;
    OutlineStyle style = OutlineStyle::Solid;
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

enum class Flow { Free, Row, Column };
enum class JustifyContent { Start, Center, End, Left, Right };
enum class AlignItems { Normal, Start, Center, End, Stretch };
enum class AlignSelf { Auto, Start, Center, End, Stretch };
enum class Overflow { Visible, Hidden };
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
enum class VerticalAlign { Top, Middle, Bottom };
enum class FontFamily { Sans };

enum class InheritedStyleProperty : uint16_t {
    NotInherited = 0,
    FontFamily = 1 << 0,
    FontSize = 1 << 1,
    FontWeight = 1 << 2,
    FontStyle = 1 << 3,
    LineHeight = 1 << 4,
    TextColor = 1 << 5,
    TextAlign = 1 << 6,
    Cursor = 1 << 7,
    LetterSpacing = 1 << 8,
    WordSpacing = 1 << 9,
    TextWrap = 1 << 10
};

using InheritedStyleProperties = uint16_t;

struct Style {
    Color backgroundColor = Color(0.f, 0.f, 0.f, 0.f);
    Color borderColor = Color(0.f, 0.f, 0.f, 1.f);
    Color textColor = Color(0.f, 0.f, 0.f, 1.f);
    Color iconStrokeColor = Color(0.f, 0.f, 0.f, 1.f);
    std::optional<Gradient> backgroundGradient;
    std::optional<Gradient> borderGradient;
    std::vector<BoxShadow> shadows;
    std::vector<Effect> effects;
    Outline outline;
    float borderRadius = 0.f;
    EdgeInsets borderWidth;
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
    bool fontStrike = false;
    TextAlign textAlign = TextAlign::Start;
    TextOverflow textOverflow = TextOverflow::Clip;
    TextWrap textWrap = TextWrap::Wrap;
    VerticalAlign verticalAlign = VerticalAlign::Top;
    bool verticalAlignSet = false;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    Flow flow = Flow::Free;
    bool flowSet = false;
    JustifyContent justifyContent = JustifyContent::Start;
    bool justifyContentSet = false;
    AlignItems alignItems = AlignItems::Normal;
    AlignSelf alignSelf = AlignSelf::Auto;
    std::optional<float> aspectRatio;
    Overflow overflowX = Overflow::Visible;
    Overflow overflowY = Overflow::Visible;
    PointerEvents pointerEvents = PointerEvents::Default;
    CursorStyle cursor = CursorStyle::Auto;
    InheritedStyleProperties specifiedInheritedProperties = 0;
};

void inheritStyle(Style& style, const Style& parent);
} // namespace radia::ui
#endif // RD_STYLE_H
