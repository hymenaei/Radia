#ifndef LL_RDUI_STYLE_H
#define LL_RDUI_STYLE_H

#include "rduitypes.h"
#include <cstdint>
#include <optional>
#include <vector>

namespace rdui
{
    struct Length
    {
        float pixels = 0.f;
        float percent = 0.f;

        float resolve(float reference) const { return pixels + percent * reference; }
        bool isPercentage() const { return percent != 0.f; }
    };

    class Dimension
    {
        public:
            static Dimension fromPixels(float pixels)
            {
                Dimension result;
                result.mLength = Length{pixels};
                return result;
            }

            static Dimension fromLength(Length length)
            {
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

    class MarginValue
    {
        public:
            MarginValue() = default;

            static MarginValue automatic()
            {
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

    class GapValue
    {
        public:
            GapValue() = default;

            static GapValue automatic()
            {
                GapValue result;
                result.mAutomatic = true;
                return result;
            }

            static GapValue fromPixels(float pixels)
            {
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

    struct MarginInsets
    {
        MarginValue top;
        MarginValue right;
        MarginValue bottom;
        MarginValue left;

        float horizontal() const { return left.fixedPixels() + right.fixedPixels(); }
        float vertical() const { return top.fixedPixels() + bottom.fixedPixels(); }
        int horizontalAutoCount() const { return static_cast<int>(left.isAuto()) + static_cast<int>(right.isAuto()); }
        int verticalAutoCount() const { return static_cast<int>(top.isAuto()) + static_cast<int>(bottom.isAuto()); }
    };

    struct GradientStop
    {
        Color color;
        float position = 0.f;
    };

    enum class GradientKind
    {
        Linear,
        Radial,
        Conic,
    };

    enum class RadialGradientShape
    {
        Ellipse,
        Circle,
    };

    struct Gradient
    {
        GradientKind kind = GradientKind::Linear;
        bool repeating = false;
        float angle_degrees = 180.f;
        Vec2 center = {.5f, .5f};
        RadialGradientShape radial_shape = RadialGradientShape::Ellipse;
        std::vector<GradientStop> stops;
    };

    struct BoxShadow
    {
        float horizontal = 0.f;
        float vertical = 0.f;
        float blur = 0.f;
        float spread = 0.f;
        Color color;
        bool inset = false;
    };

    enum class OutlineStyle
    {
        Solid,
        Dashed,
    };

    struct Outline
    {
        float width = 0.f;
        float offset = 0.f;
        Color color;
        OutlineStyle style = OutlineStyle::Solid;
    };

    enum class EffectKind
    {
        BackgroundBlur,
        LayerBlur,
    };

    struct Effect
    {
        EffectKind kind = EffectKind::LayerBlur;
        float start_radius = 0.f;
        float end_radius = 0.f;
        float angle_degrees = 180.f;

        bool progressive() const { return start_radius != end_radius; }
    };

    enum class Flow { Free, Row, Column };
    enum class JustifyContent { Start, Center, End, Left, Right };
    enum class AlignItems { Normal, Start, Center, End, Stretch };
    enum class AlignSelf { Auto, Start, Center, End, Stretch };
    enum class Overflow { Visible, Hidden };
    enum class PointerEvents { Default, Auto, PassThrough };
    enum class CursorStyle
    {
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
        Cell,
    };
    enum class TextAlign { Left, Center, Right, Start, End };
    enum class VerticalAlign { Top, Center, Bottom };
    enum class FontFamily { Sans, Small, SmallBold, SmallItalic, Medium, Big, Huge, Bold };

    enum class InheritedStyleProperty : uint16_t
    {
        NotInherited = 0,
        FontFamily = 1 << 0,
        FontSize   = 1 << 1,
        FontWeight = 1 << 2,
        FontStyle  = 1 << 3,
        LineHeight = 1 << 4,
        TextColor  = 1 << 5,
        TextAlign  = 1 << 6,
        Cursor     = 1 << 7,
    };

    using InheritedStyleProperties = uint16_t;

    struct Style
    {
        Color background_color = Color(0.f, 0.f, 0.f, 0.f);
        Color border_color = Color(0.f, 0.f, 0.f, 1.f);
        Color text_color = Color(0.f, 0.f, 0.f, 1.f);
        Color icon_stroke_color = Color(0.f, 0.f, 0.f, 1.f);
        std::optional<Gradient> background_gradient;
        std::optional<Gradient> border_gradient;
        std::vector<BoxShadow> shadows;
        std::vector<Effect> effects;
        Outline outline;
        float border_radius = 0.f;
        EdgeInsets border_width;
        std::optional<Length> svg_stroke_width;
        StrokeCap svg_stroke_cap = StrokeCap::Butt;
        bool svg_stroke_cap_set = false;
        float font_size = 13.f;
        std::optional<Length> line_height;
        float opacity = 1.f;
        Dimension width;
        Dimension height;
        std::optional<Length> min_width;
        std::optional<Length> min_height;
        std::optional<Length> left;
        std::optional<Length> right;
        std::optional<Length> top;
        std::optional<Length> bottom;
        MarginInsets margin;
        EdgeInsets padding;
        GapValue gap;
        float grow = 0.f;
        int order = 0;
        FontFamily font_family = FontFamily::Sans;
        bool font_bold = false;
        bool font_italic = false;
        TextAlign text_align = TextAlign::Start;
        VerticalAlign vertical_align = VerticalAlign::Top;
        Flow flow = Flow::Free;
        JustifyContent justify_content = JustifyContent::Start;
        AlignItems align_items = AlignItems::Normal;
        AlignSelf align_self = AlignSelf::Auto;
        std::optional<float> aspect_ratio;
        Overflow overflow = Overflow::Visible;
        PointerEvents pointer_events = PointerEvents::Default;
        CursorStyle cursor = CursorStyle::Auto;
        InheritedStyleProperties specified_inherited = 0;
    };

    void inheritStyle(Style& style, const Style& parent);
}

#endif // LL_RDUI_STYLE_H
