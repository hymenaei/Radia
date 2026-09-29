/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "OpenGLPaintContext.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <deque>
#include <limits>
#include <map>
#include <numbers>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <tuple>
#include <unordered_map>
#include <utility>
#include <variant>
#include <vector>
#include <fmt/format.h>
#include "BorderImageGrid.h"
#include "LayoutGeometry.h"
#include "OpenGLPaintState.h"
#include "RasterImage.h"
#include "SVGImage.h"
#include "StyleSheet.h"
#include "System.h"
#include "SystemFontProvider.h"
#include "Tessellator.h"
#include "llfontfreetype.h"
#include "llfontgl.h"
#include "llgl.h"
#include "llglslshader.h"
#include "llgltexture.h"
#include "llimage.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llshadermgr.h"
#include "llstring.h"
#include "v4color.h"

namespace Core {
namespace {
Layout::Rect snapped(const Layout::Rect& rect) {
    const float left = std::round(rect.left());
    const float right = std::round(rect.right());
    const float bottom = std::round(rect.bottom());
    const float top = std::round(rect.top());
    return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

Layout::Rect snappedScrollbarArrow(const NativeScrollbarAxisGeometry& axis, bool start) {
    const Layout::Rect arrow = start ? axis.startArrow : axis.endArrow;
    if (!axis.visible || arrow.empty())
        return {};

    const Layout::Rect bounds = snapped(axis.bounds);
    if (axis.axis == ScrollbarAxis::Horizontal) {
        const float length = std::max(0.f, std::round(axis.startArrow.w));
        const bool startOnLeft = axis.startArrow.left() < axis.endArrow.left();
        const bool onLeft = start ? startOnLeft : !startOnLeft;
        return {onLeft ? bounds.left() : bounds.right() - length, bounds.y, length, bounds.h};
    }

    const float length = std::max(0.f, std::round(axis.startArrow.h));
    const bool startOnTop = axis.startArrow.bottom() > axis.endArrow.bottom();
    const bool onTop = start ? startOnTop : !startOnTop;
    return {bounds.x, onTop ? bounds.top() - length : bounds.bottom(), bounds.w, length};
}

bool hasVisibleBorder(const Style::ComputedStyle& style) {
    const Layout::RectEdges<float> widths = Layout::borderWidths(style);
    const Layout::RectEdges<Style::Color> colors = style.borderColor();
    return (widths.top > 0.f && colors.top.resolvedColor().a > 0.f) || (widths.right > 0.f && colors.right.resolvedColor().a > 0.f)
        || (widths.bottom > 0.f && colors.bottom.resolvedColor().a > 0.f) || (widths.left > 0.f && colors.left.resolvedColor().a > 0.f);
}

Color shade(Color source, Color target, float amount) {
    return {source.r + (target.r - source.r) * amount, source.g + (target.g - source.g) * amount, source.b + (target.b - source.b) * amount,
        source.a};
}

struct ResolvedBorderRadius {
    Layout::Vec2 topLeft;
    Layout::Vec2 topRight;
    Layout::Vec2 bottomRight;
    Layout::Vec2 bottomLeft;
};

Layout::Vec2 resolveCornerRadius(const Style::CornerRadius& radius, float width, float height) {
    return {std::max(0.f, radius.horizontal.resolve(width)), std::max(0.f, radius.vertical.resolve(height))};
}

ResolvedBorderRadius normalizeBorderRadius(ResolvedBorderRadius radii, float width, float height) {
    float scale = 1.f;
    const auto limit = [&scale](float available, float sum) {
        if (sum > 0.f)
            scale = std::min(scale, std::max(0.f, available) / sum);
    };
    limit(width, radii.topLeft.x + radii.topRight.x);
    limit(width, radii.bottomLeft.x + radii.bottomRight.x);
    limit(height, radii.topLeft.y + radii.bottomLeft.y);
    limit(height, radii.topRight.y + radii.bottomRight.y);
    radii.topLeft = radii.topLeft * scale;
    radii.topRight = radii.topRight * scale;
    radii.bottomRight = radii.bottomRight * scale;
    radii.bottomLeft = radii.bottomLeft * scale;
    return radii;
}

ResolvedBorderRadius resolveBorderRadius(const Layout::Rect& rect, const Style::BorderRadius& source) {
    const float width = std::max(0.f, rect.w);
    const float height = std::max(0.f, rect.h);
    return normalizeBorderRadius({resolveCornerRadius(source.topLeft, width, height), resolveCornerRadius(source.topRight, width, height),
                                     resolveCornerRadius(source.bottomRight, width, height),
                                     resolveCornerRadius(source.bottomLeft, width, height)},
        width, height);
}

ResolvedBorderRadius insetBorderRadius(const ResolvedBorderRadius& radii, const Layout::RectEdges<float>& inset, float width,
    float height) {
    return normalizeBorderRadius(
        {
            {std::max(0.f, radii.topLeft.x - inset.left), std::max(0.f, radii.topLeft.y - inset.top)},
            {std::max(0.f, radii.topRight.x - inset.right), std::max(0.f, radii.topRight.y - inset.top)},
            {std::max(0.f, radii.bottomRight.x - inset.right), std::max(0.f, radii.bottomRight.y - inset.bottom)},
            {std::max(0.f, radii.bottomLeft.x - inset.left), std::max(0.f, radii.bottomLeft.y - inset.bottom)},
        },
        width, height);
}

ResolvedBorderRadius expandedBorderRadius(const ResolvedBorderRadius& radii, float amount, float width, float height) {
    return normalizeBorderRadius({radii.topLeft + Layout::Vec2(amount, amount), radii.topRight + Layout::Vec2(amount, amount),
                                     radii.bottomRight + Layout::Vec2(amount, amount), radii.bottomLeft + Layout::Vec2(amount, amount)},
        width, height);
}

ResolvedBorderRadius uniformBorderRadius(float radius) {
    const Layout::Vec2 corner(std::max(0.f, radius), std::max(0.f, radius));
    return {corner, corner, corner, corner};
}

struct ResolvedScrollbarClip {
    Layout::Rect rect;
    ResolvedBorderRadius radii;
};

std::optional<ResolvedScrollbarClip> resolveScrollbarClip(const NativeScrollbarClip& source) {
    if (!source.enabled || source.borderBox.empty())
        return std::nullopt;
    const Layout::Rect innerBox = Layout::insetRect(source.borderBox, source.borderWidth);
    if (innerBox.empty())
        return std::nullopt;
    const ResolvedBorderRadius outerRadii = resolveBorderRadius(source.borderBox, source.borderRadius);
    return ResolvedScrollbarClip {innerBox, insetBorderRadius(outerRadii, source.borderWidth, innerBox.w, innerBox.h)};
}

Layout::Rect expandedRect(const Layout::Rect& rect, float amount) {
    return {rect.x - amount, rect.y - amount, rect.w + amount * 2.f, rect.h + amount * 2.f};
}

Layout::Rect backgroundBox(const Layout::Rect& borderBox, const Style::ComputedStyle& style, Style::BackgroundBox box) {
    if (box == Style::BackgroundBox::BorderBox)
        return borderBox;
    const Layout::Rect paddingBox = Layout::insetRect(borderBox, Layout::borderWidths(style));
    return box == Style::BackgroundBox::PaddingBox ? paddingBox : Layout::insetRect(paddingBox, Layout::paddingPixels(style));
}

ResolvedBorderRadius backgroundBoxRadii(const Layout::Rect& borderBox, const Style::ComputedStyle& style, Style::BackgroundBox box) {
    const ResolvedBorderRadius borderRadii = resolveBorderRadius(borderBox, style.borderRadius());
    if (box == Style::BackgroundBox::BorderBox)
        return borderRadii;
    const Layout::RectEdges<float> borderInsets = Layout::borderWidths(style);
    const Layout::Rect paddingBox = Layout::insetRect(borderBox, borderInsets);
    const ResolvedBorderRadius paddingRadii = insetBorderRadius(borderRadii, borderInsets, paddingBox.w, paddingBox.h);
    if (box == Style::BackgroundBox::PaddingBox)
        return paddingRadii;
    const Layout::Rect contentBox = Layout::insetRect(paddingBox, Layout::paddingPixels(style));
    return insetBorderRadius(paddingRadii, Layout::paddingPixels(style), contentBox.w, contentBox.h);
}

float coverageFringeWidth(float scale) { return scale > .0001f ? 1.f / scale : 1.f; }

Layout::Rect snappedOutward(const Layout::Rect& rect, float scale) {
    const float safeScale = std::max(scale, .0001f);
    const float left = std::floor(rect.left() * safeScale) / safeScale;
    const float right = std::ceil(rect.right() * safeScale) / safeScale;
    const float bottom = std::floor(rect.bottom() * safeScale) / safeScale;
    const float top = std::ceil(rect.top() * safeScale) / safeScale;
    return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

bool ensureTarget(LLRenderTarget& target, U32 width, U32 height) {
    width = std::max<U32>(1, width);
    height = std::max<U32>(1, height);
    if (!target.isComplete())
        return target.allocate(width, height, GL_RGBA8);
    if (target.getWidth() != width || target.getHeight() != height)
        target.resize(width, height);
    return target.isComplete() && target.getWidth() == width && target.getHeight() == height;
}

LLFontGL::HAlign horizontalAlignment(const Style::ComputedStyle& style) {
    return style.textAlign() == Style::TextAlign::Center ? LLFontGL::HCENTER
        : style.textAlign() == Style::TextAlign::Right   ? LLFontGL::RIGHT
                                                         : LLFontGL::LEFT;
}

float textX(const Layout::Rect& rect, LLFontGL::HAlign align) {
    return align == LLFontGL::HCENTER ? rect.x + rect.w * 0.5f : align == LLFontGL::RIGHT ? rect.right() : rect.x;
}

float textY(const Layout::Rect& rect, LLFontGL::VAlign align) {
    return align == LLFontGL::VCENTER ? rect.y + rect.h * 0.5f : align == LLFontGL::BOTTOM ? rect.y : rect.top();
}

float textBaseline(const Layout::Rect& rect, LLFontGL::VAlign align, const LLFontGL& font) {
    const float anchor = textY(rect, align);
    if (align == LLFontGL::TOP)
        return anchor - font.getAscenderHeight();
    if (align == LLFontGL::BOTTOM)
        return anchor + font.getDescenderHeight();
    if (align == LLFontGL::VCENTER)
        return anchor - (font.getAscenderHeight() - font.getDescenderHeight()) * .5f;
    return anchor;
}

LLFontGL::TextSpacing usedTextSpacing(const Style::ComputedStyle& style) {
    return {
        style.letterSpacing().resolve(style.fontSize()),
        style.wordSpacing().resolve(style.fontSize()),
    };
}

const char* genericFontFamilyName(Style::GenericFontFamily family) {
    switch (family) {
    case Style::GenericFontFamily::Serif:
        return "serif";
    case Style::GenericFontFamily::SansSerif:
        return "sans-serif";
    case Style::GenericFontFamily::SystemUI:
        return "system-ui";
    case Style::GenericFontFamily::Cursive:
        return "cursive";
    case Style::GenericFontFamily::Fantasy:
        return "fantasy";
    case Style::GenericFontFamily::Math:
        return "math";
    case Style::GenericFontFamily::Monospace:
        return "monospace";
    case Style::GenericFontFamily::UISerif:
        return "ui-serif";
    case Style::GenericFontFamily::UISansSerif:
        return "ui-sans-serif";
    case Style::GenericFontFamily::UIMonospace:
        return "ui-monospace";
    case Style::GenericFontFamily::UIRounded:
        return "ui-rounded";
    case Style::GenericFontFamily::Fangsong:
        return "fangsong";
    case Style::GenericFontFamily::Kai:
        return "kai";
    case Style::GenericFontFamily::KhmerMul:
        return "khmer-mul";
    case Style::GenericFontFamily::Nastaliq:
        return "nastaliq";
    }
    return "unknown";
}

std::string describeFontFamily(const Style::FontFamily& family) {
    if (const auto* name = std::get_if<std::string>(&family))
        return "'" + *name + "'";
    return genericFontFamilyName(std::get<Style::GenericFontFamily>(family));
}

std::string describeFontFamilies(const Style::FontFamilies& families) {
    std::string result;
    for (const Style::FontFamily& family : families) {
        if (!result.empty())
            result += ", ";
        result += describeFontFamily(family);
    }
    return result;
}

void drawTexturedQuad(const Layout::Rect& rect, float u0 = 0.f, float v0 = 0.f, float u1 = 1.f, float v1 = 1.f) {
    gGL.begin(LLRender::TRIANGLES);
    gGL.color4f(1.f, 1.f, 1.f, 1.f);
    gGL.texCoord2f(u0, v0);
    gGL.vertex2f(rect.left(), rect.bottom());
    gGL.texCoord2f(u1, v0);
    gGL.vertex2f(rect.right(), rect.bottom());
    gGL.texCoord2f(u1, v1);
    gGL.vertex2f(rect.right(), rect.top());
    gGL.texCoord2f(u0, v0);
    gGL.vertex2f(rect.left(), rect.bottom());
    gGL.texCoord2f(u1, v1);
    gGL.vertex2f(rect.right(), rect.top());
    gGL.texCoord2f(u0, v1);
    gGL.vertex2f(rect.left(), rect.top());
    gGL.end();
}

enum class PaintOp : GLint {
#define GRADIENT_OP_ENTRY(name, value)
#define OUTLINE_OP_ENTRY(name, value)
#define PAINT_OP_ENTRY(name, value) name = value,
#include "PaintProtocol.def"
#undef PAINT_OP_ENTRY
#undef GRADIENT_OP_ENTRY
#undef OUTLINE_OP_ENTRY
};

enum class GradientOp : GLint {
#define PAINT_OP_ENTRY(name, value)
#define GRADIENT_OP_ENTRY(name, value) name = value,
#define OUTLINE_OP_ENTRY(name, value)
#include "PaintProtocol.def"
#undef PAINT_OP_ENTRY
#undef GRADIENT_OP_ENTRY
#undef OUTLINE_OP_ENTRY
};

enum class OutlineOp : GLint {
#define PAINT_OP_ENTRY(name, value)
#define GRADIENT_OP_ENTRY(name, value)
#define OUTLINE_OP_ENTRY(name, value) name = value,
#include "PaintProtocol.def"
#undef PAINT_OP_ENTRY
#undef GRADIENT_OP_ENTRY
#undef OUTLINE_OP_ENTRY
};

struct PaintShaderUniforms {
    LLStaticHashedString paintOp {"paintOp"};
    LLStaticHashedString shapeRect {"shapeRect"};
    LLStaticHashedString shapeRadiusX {"shapeRadiusX"};
    LLStaticHashedString shapeRadiusY {"shapeRadiusY"};
    LLStaticHashedString innerRadiusX {"innerRadiusX"};
    LLStaticHashedString innerRadiusY {"innerRadiusY"};
    LLStaticHashedString scrollbarClipRect {"scrollbarClipRect"};
    LLStaticHashedString scrollbarClipRadiusX {"scrollbarClipRadiusX"};
    LLStaticHashedString scrollbarClipRadiusY {"scrollbarClipRadiusY"};
    LLStaticHashedString scrollbarClipEnabled {"scrollbarClipEnabled"};
    LLStaticHashedString shapeBorderWidth {"shapeBorderWidth"};
    LLStaticHashedString shapeColor {"shapeColor"};
    LLStaticHashedString shapeOffset {"shapeOffset"};
    LLStaticHashedString arrowDirection {"arrowDirection"};
    LLStaticHashedString outlineStyle {"outlineStyle"};
    LLStaticHashedString borderWidths {"borderWidths"};
    LLStaticHashedString topBorderGap {"topBorderGap"};
    LLStaticHashedString gradientKind {"gradientKind"};
    LLStaticHashedString gradientRepeating {"gradientRepeating"};
    LLStaticHashedString gradientStart {"gradientStart"};
    LLStaticHashedString gradientEnd {"gradientEnd"};
    LLStaticHashedString gradientTransform {"gradientTransform"};
    LLStaticHashedString gradientCenter {"gradientCenter"};
    LLStaticHashedString gradientRadius {"gradientRadius"};
    LLStaticHashedString gradientAngle {"gradientAngle"};
    LLStaticHashedString gradientStopCount {"gradientStopCount"};
    LLStaticHashedString gradientColors {"gradientColors"};
    LLStaticHashedString gradientStops {"gradientStops"};
    LLStaticHashedString shadowOffset {"shadowOffset"};
    LLStaticHashedString shadowBlur {"shadowBlur"};
    LLStaticHashedString shadowSpread {"shadowSpread"};
    LLStaticHashedString effectTextureSize {"effectTextureSize"};
    LLStaticHashedString effectBlurAxis {"effectBlurAxis"};
    LLStaticHashedString effectBlurRadii {"effectBlurRadii"};
    LLStaticHashedString effectGradientStart {"effectGradientStart"};
    LLStaticHashedString effectGradientEnd {"effectGradientEnd"};
    LLStaticHashedString effectCaptureRect {"effectCaptureRect"};
    LLStaticHashedString effectMaskRect {"effectMaskRect"};
    LLStaticHashedString effectMaskRadiusX {"effectMaskRadiusX"};
    LLStaticHashedString effectMaskRadiusY {"effectMaskRadiusY"};
    LLStaticHashedString effectRoundedMask {"effectRoundedMask"};
    LLStaticHashedString maskMode {"maskMode"};
    LLStaticHashedString clipCoverageRect {"clipCoverageRect"};
    LLStaticHashedString clipCoverageEnabled {"clipCoverageEnabled"};
    LLStaticHashedString roundedClipRect {"roundedClipRect"};
    LLStaticHashedString roundedClipRadiusX {"roundedClipRadiusX"};
    LLStaticHashedString roundedClipRadiusY {"roundedClipRadiusY"};
    LLStaticHashedString roundedClipEnabled {"roundedClipEnabled"};
};

const PaintShaderUniforms& shaderUniforms() {
    static const PaintShaderUniforms sUniforms;
    return sUniforms;
}

GLint gradientOpValue(Style::GradientKind kind) {
    switch (kind) {
    case Style::GradientKind::Linear:
        return static_cast<GLint>(GradientOp::Linear);
    case Style::GradientKind::Radial:
        return static_cast<GLint>(GradientOp::Radial);
    case Style::GradientKind::Conic:
        return static_cast<GLint>(GradientOp::Conic);
    }
    llassert(false);
    return static_cast<GLint>(GradientOp::Linear);
}

GLint outlineOpValue(Style::OutlineStyle style) {
    switch (style) {
    case Style::OutlineStyle::Solid:
        return static_cast<GLint>(OutlineOp::Solid);
    case Style::OutlineStyle::Dashed:
        return static_cast<GLint>(OutlineOp::Dashed);
    }
    llassert(false);
    return static_cast<GLint>(OutlineOp::Solid);
}

void setPaintOp(LLGLSLShader& program, PaintOp op) { program.uniform1i(shaderUniforms().paintOp, static_cast<GLint>(op)); }

void setClipCoverageUniforms(LLGLSLShader& program, const std::optional<Layout::Rect>& bounds) {
    const PaintShaderUniforms& uniforms = shaderUniforms();
    if (!bounds || bounds->empty()) {
        program.uniform1i(uniforms.clipCoverageEnabled, 0);
        return;
    }
    program.uniform1i(uniforms.clipCoverageEnabled, 1);
    program.uniform4f(uniforms.clipCoverageRect, bounds->x, bounds->y, bounds->w, bounds->h);
}

void setBorderRadiusUniforms(LLGLSLShader& program, const PaintShaderUniforms& uniforms, const ResolvedBorderRadius& radii, bool inner) {
    const LLStaticHashedString& radiusX = inner ? uniforms.innerRadiusX : uniforms.shapeRadiusX;
    const LLStaticHashedString& radiusY = inner ? uniforms.innerRadiusY : uniforms.shapeRadiusY;
    program.uniform4f(radiusX, radii.topLeft.x, radii.topRight.x, radii.bottomRight.x, radii.bottomLeft.x);
    program.uniform4f(radiusY, radii.topLeft.y, radii.topRight.y, radii.bottomRight.y, radii.bottomLeft.y);
}

void setScrollbarClipUniforms(LLGLSLShader& program, const PaintShaderUniforms& uniforms, const Layout::Rect& shapeRect,
    const ResolvedScrollbarClip* clip) {
    if (!clip) {
        program.uniform1i(uniforms.scrollbarClipEnabled, 0);
        return;
    }
    program.uniform1i(uniforms.scrollbarClipEnabled, 1);
    program.uniform4f(uniforms.scrollbarClipRect, clip->rect.x - shapeRect.x, clip->rect.y - shapeRect.y, clip->rect.w, clip->rect.h);
    program.uniform4f(uniforms.scrollbarClipRadiusX, clip->radii.topLeft.x, clip->radii.topRight.x, clip->radii.bottomRight.x,
        clip->radii.bottomLeft.x);
    program.uniform4f(uniforms.scrollbarClipRadiusY, clip->radii.topLeft.y, clip->radii.topRight.y, clip->radii.bottomRight.y,
        clip->radii.bottomLeft.y);
}

void setGradientUniforms(LLGLSLShader& program, const PaintShaderUniforms& uniforms, const Layout::Rect& rect,
    const Style::Gradient& gradient, float scaleX = 1.f, float scaleY = 1.f, float offsetX = 0.f, float offsetY = 0.f) {
    constexpr float kRadiansPerDegree = std::numbers::pi_v<float> / 180.f;
    const float angle = gradient.angleDegrees * kRadiansPerDegree;
    Layout::Vec2 direction(std::sin(angle), std::cos(angle));
    if (gradient.cornerDirection)
        direction = Layout::normalize({std::copysign(rect.w, direction.x), std::copysign(rect.h, direction.y)});
    const float extent = std::abs(direction.x) * rect.w + std::abs(direction.y) * rect.h;
    const Layout::Vec2 center(rect.w * .5f, rect.h * .5f);
    const Layout::Vec2 start = center - direction * (extent * .5f);
    const Layout::Vec2 end = center + direction * (extent * .5f);
    const Layout::Vec2 gradientCenterValue(gradient.center.x * rect.w, gradient.center.y * rect.h);
    const float farX = std::max(gradientCenterValue.x, rect.w - gradientCenterValue.x);
    const float farY = std::max(gradientCenterValue.y, rect.h - gradientCenterValue.y);
    Layout::Vec2 radialRadius;
    if (gradient.radialShape == Style::RadialGradientShape::Circle) {
        const float farCorner = std::sqrt(farX * farX + farY * farY);
        radialRadius = {farCorner, farCorner};
    } else {
        constexpr float kSquareRootTwo = 1.41421356237f;
        radialRadius = {farX * kSquareRootTwo, farY * kSquareRootTwo};
    }
    constexpr std::size_t kMaxGradientStops = 8;
    std::array<GLfloat, kMaxGradientStops * 4> colors {};
    std::array<GLfloat, kMaxGradientStops> stops {};
    for (std::size_t index = 0; index < gradient.stops.size(); ++index) {
        const Style::GradientStop& stop = gradient.stops[index];
        const Color& color = stop.color.resolvedColor();
        colors[index * 4] = color.r;
        colors[index * 4 + 1] = color.g;
        colors[index * 4 + 2] = color.b;
        colors[index * 4 + 3] = color.a;
        stops[index] = stop.position;
    }

    program.uniform1i(uniforms.gradientKind, gradientOpValue(gradient.kind));
    program.uniform1i(uniforms.gradientRepeating, gradient.repeating ? 1 : 0);
    program.uniform2f(uniforms.gradientStart, start.x, start.y);
    program.uniform2f(uniforms.gradientEnd, end.x, end.y);
    program.uniform4f(uniforms.gradientTransform, scaleX, scaleY, offsetX, offsetY);
    program.uniform2f(uniforms.gradientCenter, gradientCenterValue.x, gradientCenterValue.y);
    program.uniform2f(uniforms.gradientRadius, radialRadius.x, radialRadius.y);
    program.uniform1f(uniforms.gradientAngle, gradient.angleDegrees);
    program.uniform1i(uniforms.gradientStopCount, static_cast<GLint>(gradient.stops.size()));
    program.uniform4fv(uniforms.gradientColors, static_cast<U32>(gradient.stops.size()), colors.data());
    program.uniform1fv(uniforms.gradientStops, static_cast<U32>(gradient.stops.size()), stops.data());
}
} // namespace

struct GeometryPainter {
    GeometryPainter(::LLGLSLShader& shapeProgram, detail::ClipStack& clipStack, const System& system)
        : program(shapeProgram)
        , clips(clipStack)
        , system(system) {}

    void beginFrame(const PaintTarget& target) {
        mTargetScale = target.scale;
        if (mResourceGeneration != system.generation()) {
            rasterTextures.clear();
            mResourceGeneration = system.generation();
        }
    }
    void drawMesh(const Mesh& mesh);
    void drawGradientMesh(const Mesh& mesh, const Layout::Rect& rect, const Style::Gradient& gradient);
    void drawBorderImageGradientTile(const Layout::Rect& tile, const Layout::Rect& source, float imageWidth, float imageHeight,
        const Style::Gradient& gradient, float opacity);
    void drawArrow(const Layout::Rect& rect, ScrollbarAxis axis, bool pointsPositive, const Color& color,
        const ResolvedScrollbarClip* clip = nullptr);
    void drawNativeInputMark(const NativeInputMarkPaintRequest& request);
    void drawRoundedShape(PaintOp op, const Layout::Rect& rect, const ResolvedBorderRadius& radii, float borderWidth, const Color& color,
        Style::OutlineStyle outlineStyle = Style::OutlineStyle::Solid, std::optional<TopBorderGap> topBorderGap = std::nullopt,
        const ResolvedBorderRadius* innerRadii = nullptr, const ResolvedScrollbarClip* clip = nullptr);
    void drawRoundedGradient(const Layout::Rect& rect, const ResolvedBorderRadius& radii, const Style::Gradient& gradient,
        const Layout::RectEdges<float>* borderWidths = nullptr, std::optional<TopBorderGap> topBorderGap = std::nullopt,
        const ResolvedBorderRadius* innerRadii = nullptr);
    void drawShadow(const Layout::Rect& rect, const ResolvedBorderRadius& radii, const Style::BoxShadow& shadow);
    bool drawBorderImage(const Layout::Rect& rect, const Style::ComputedStyle& style);
    void drawBorder(const Layout::Rect& rect, const Style::ComputedStyle& style, std::optional<TopBorderGap> topBorderGap = std::nullopt);
    void drawOutline(const Layout::Rect& rect, const Style::ComputedStyle& style);
    void paintImageLayers(const Layout::Rect& rect, const Style::ComputedStyle& style, std::span<const Style::BackgroundLayer> layers,
        const Color& vectorColor, const Style::Gradient* vectorGradient = nullptr,
        const BackgroundPaintContext* backgroundContext = nullptr);
    void paintMaskLayers(const Layout::Rect& rect, const Style::ComputedStyle& style, std::span<const Style::MaskLayer> layers);
    bool hasRenderableMask(std::span<const Style::MaskLayer> layers) const;
    void paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style, std::optional<TopBorderGap> topBorderGap,
        const BackgroundPaintContext* backgroundContext = nullptr);
    void prepareVectorDraw();
    void applyClipCoverage(::LLGLSLShader& target) const { setClipCoverageUniforms(target, clips.coverageBounds()); }
    void setRoundedClip(const ResolvedBorderRadius& radii);
    void clearRoundedClip();
    const SVGImage* resourceSvg(const Style::BackgroundLayer& layer) const;
    const RasterImage* resourceRaster(const Style::BackgroundLayer& layer) const;
    LLGLTexture* rasterTexture(std::string_view reference);
    static void drawShapeQuad(const Layout::Rect& rect, float alpha = 1.f);
    float coverageFringe() const { return coverageFringeWidth(mTargetScale); }

    ::LLGLSLShader& program;
    detail::ClipStack& clips;
    const System& system;
    float mTargetScale = 1.f;
    std::uint64_t mResourceGeneration = 0;
    std::unordered_map<std::string, LLPointer<LLGLTexture>> rasterTextures;
};

struct TextPainter {
    struct FontInstanceKey {
        std::string source;
        S32 faceIndex;
        float pixelSize;
        float weight;
        float width;
        Style::FontStyle style;
        Style::FontFamilies familyList;
        bool isFallback;

        bool operator<(const FontInstanceKey& other) const {
            return std::tie(source, faceIndex, pixelSize, weight, width, style, familyList, isFallback) < std::tie(other.source,
                       other.faceIndex, other.pixelSize, other.weight, other.width, other.style, other.familyList, other.isFallback);
        }
    };

    explicit TextPainter(GeometryPainter& geometryPainter)
        : geometry(geometryPainter) {}

    Layout::Vec2 measureText(const std::string& text, const Style::ComputedStyle& style);
    float usedLetterSpacing(const Style::ComputedStyle& style) const;
    void paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style);
    void destroyGL();
    static void prepareTextDraw();

    const LLFontGL* fontForStyle(const Style::ComputedStyle& style);
    float lineHeight(const Style::ComputedStyle& style, const LLFontGL* font = nullptr);
    const LLFontGL* fontForFace(const CSS::FontFace& face, float pixelSize, const Style::FontSelectionRequest& request, bool isFallback,
        const Style::FontFamilies& familyList);
    const LLFontGL* fontForSource(const detail::SystemFontMatch& source, float pixelSize, const Style::FontSelectionRequest& request,
        bool isFallback, const Style::FontFamilies& familyList, std::string_view selectedFamily, std::string_view sourceKind,
        std::string_view sourceName, std::string_view reason);
    std::optional<detail::SystemFontMatch> systemFont(const Style::FontFamily& family, const Style::FontSelectionRequest& request);
    std::optional<detail::SystemFontMatch> localFont(std::string_view name);
    void clearFonts();

    GeometryPainter& geometry;
    std::unordered_map<std::string, std::string> memoryFontKeys;
    std::map<std::string, std::optional<detail::SystemFontMatch>> localFontMatches;
    std::map<std::tuple<Style::FontFamily, float, float, Style::FontStyle>, std::optional<detail::SystemFontMatch>> systemFontMatches;
    std::map<FontInstanceKey, std::unique_ptr<LLFontGL>> fontInstances;
    std::uint64_t resourceGeneration = 0;
    S32 resolutionGeneration = -1;
};

struct BlurProfile {
    float angleDegrees = 180.f;
    Style::BlurStop start;
    Style::BlurStop end;
};

std::optional<BlurProfile> resolveBlurProfile(const Style::BlurFilter& filter) {
    return BlurProfile {180.f, {filter.stdDeviation, 0.f}, {filter.stdDeviation, 1.f}};
}

std::optional<BlurProfile> resolveBlurProfile(const Style::LinearBlurFilter& filter) {
    if (filter.stops.size() != 2)
        return std::nullopt;
    return BlurProfile {filter.angleDegrees, filter.stops[0], filter.stops[1]};
}

std::optional<BlurProfile> resolveBlurProfile(const Style::FilterOperation& operation) {
    return std::visit(
        [](const auto& filter) {
            return resolveBlurProfile(filter);
        },
        operation);
}

std::optional<float> maximumBlurDeviation(const Style::FilterOperation& operation) {
    const std::optional<BlurProfile> profile = resolveBlurProfile(operation);
    if (!profile)
        return std::nullopt;
    return std::max(profile->start.stdDeviation, profile->end.stdDeviation);
}

class EffectRenderer final {
    struct EffectLayer {
        std::array<LLRenderTarget, 3> targets;
        LLRenderTarget maskTarget;
        Style::Filter filter;
        std::vector<Style::MaskLayer> maskLayers;
        Style::ComputedStyle maskStyle;
        Layout::Rect effectRect;
        Layout::Rect captureRect;
        float scale = 1.f;
        bool hasMask = false;
        std::optional<detail::EffectCaptureGuard> capture;
    };

public:
    EffectRenderer(::LLGLSLShader& shapeProgram, detail::ClipStack& clipStack, GeometryPainter& geometry)
        : mProgram(shapeProgram)
        , mClips(clipStack)
        , mGeometry(geometry) {}

    void begin(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale);
    void end();
    void resetFrame() {
        while (mEffectDepth > 0)
            mEffectLayers[--mEffectDepth].capture.reset();
        for (EffectLayer& layer : mEffectLayers)
            layer.capture.reset();
        mEffectDepth = 0;
    }
    std::size_t depth() const { return mEffectDepth; }

private:
    bool captureFramebuffer(const Layout::Rect& capture, float scale, LLRenderTarget& target);
    LLRenderTarget* applyBlur(LLRenderTarget& source, LLRenderTarget& horizontalTarget, LLRenderTarget& verticalTarget,
        const Layout::Rect& capture, const Layout::Rect& effectRect, const Style::FilterOperation& operation, float scale);
    void compositeEffect(LLRenderTarget& source, const Layout::Rect& capture, const Layout::Rect& destination,
        const ResolvedBorderRadius& radii, bool roundedMask);
    void compositeMaskedEffect(LLRenderTarget& source, LLRenderTarget& mask, const Layout::Rect& capture);

    ::LLGLSLShader& mProgram;
    detail::ClipStack& mClips;
    GeometryPainter& mGeometry;
    std::array<LLRenderTarget, 3> mBackgroundTargets;
    std::deque<EffectLayer> mEffectLayers;
    std::size_t mEffectDepth = 0;
};

struct OpenGLPaintContext::Impl {
    Impl(::LLGLSLShader& shapeProgram, const System& system)
        : system(system)
        , geometry(shapeProgram, clipStack, system)
        , text(geometry)
        , effects(shapeProgram, clipStack, geometry) {}

    void beginFrame(const PaintTarget& target);
    void endFrame();
    Layout::Vec2 measureText(const std::string& text, const Style::ComputedStyle& style);
    float usedLetterSpacing(const Style::ComputedStyle& style) const;
    void pushClip(const Layout::Rect& rect, float scale, Layout::ClipAxes axes);
    void popClip();
    void pushTranslation(const Layout::Vec2& translation);
    void popTranslation();
    void beginEffects(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale);
    void endEffects();
    const NativeAppearance& nativeAppearance() const {
        return mFrameNativeAppearance ? *mFrameNativeAppearance : system.nativeAppearance();
    }
    void paintNativeScrollbar(const NativeScrollbarPaintRequest& request);
    void paintNativeInputMark(const NativeInputMarkPaintRequest& request);
    void paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style, std::optional<TopBorderGap> topBorderGap,
        const BackgroundPaintContext* backgroundContext = nullptr);
    void paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style);

    std::unique_ptr<LLGLSUIDefault> uiState;
    std::optional<LLGLSColorMask> colorMask;
    const System& system;
    mutable std::optional<std::pair<std::uint64_t, S32>> observedTextGeneration;
    mutable std::uint64_t textGeneration = 0;
    const NativeAppearance* mFrameNativeAppearance = nullptr;
    detail::ClipStack clipStack;
    GeometryPainter geometry;
    TextPainter text;
    EffectRenderer effects;

    std::uint64_t generation() const noexcept {
        const auto current = std::pair {system.generation(), LLFontGL::sResolutionGeneration};
        if (!observedTextGeneration || *observedTextGeneration != current) {
            observedTextGeneration = current;
            ++textGeneration;
        }
        return textGeneration;
    }
};

OpenGLPaintContext::OpenGLPaintContext(::LLGLSLShader& shapeProgram, const System& system)
    : mImpl(std::make_unique<Impl>(shapeProgram, system)) {}

OpenGLPaintContext::~OpenGLPaintContext() = default;

void OpenGLPaintContext::beginFrame(const PaintTarget& target) { mImpl->beginFrame(target); }

void OpenGLPaintContext::endFrame() { mImpl->endFrame(); }

Layout::Vec2 OpenGLPaintContext::measureText(const std::string& text, const Style::ComputedStyle& style) const {
    return mImpl->measureText(text, style);
}

float OpenGLPaintContext::usedLetterSpacing(const Style::ComputedStyle& style) const { return mImpl->usedLetterSpacing(style); }

void OpenGLPaintContext::destroyGL() { mImpl->text.destroyGL(); }

std::uint64_t OpenGLPaintContext::generation() const noexcept { return mImpl->generation(); }

void OpenGLPaintContext::pushClip(const Layout::Rect& rect, float scale, Layout::ClipAxes axes) { mImpl->pushClip(rect, scale, axes); }

void OpenGLPaintContext::popClip() { mImpl->popClip(); }

void OpenGLPaintContext::pushTranslation(const Layout::Vec2& translation) { mImpl->pushTranslation(translation); }

void OpenGLPaintContext::popTranslation() { mImpl->popTranslation(); }

void OpenGLPaintContext::beginEffects(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale) {
    mImpl->beginEffects(rect, style, scale);
}

void OpenGLPaintContext::endEffects() { mImpl->endEffects(); }

void OpenGLPaintContext::paintNativeScrollbar(const NativeScrollbarPaintRequest& request) { mImpl->paintNativeScrollbar(request); }

void OpenGLPaintContext::paintNativeInput(const NativeInputPaintRequest& request) { mImpl->nativeAppearance().paintInput(*this, request); }

void OpenGLPaintContext::paintNativeInputMark(const NativeInputMarkPaintRequest& request) { mImpl->paintNativeInputMark(request); }

void OpenGLPaintContext::paintNativeButton(const NativeButtonPaintRequest& request) {
    mImpl->nativeAppearance().paintButton(*this, request);
}

void OpenGLPaintContext::paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style, std::optional<TopBorderGap> topBorderGap) {
    const auto& context = backgroundPaintContext();
    mImpl->paintBox(rect, style, topBorderGap, context ? &*context : nullptr);
}

void OpenGLPaintContext::paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style) {
    mImpl->paintText(text, rect, style);
}

Layout::Vec2 TextPainter::measureText(const std::string& text, const Style::ComputedStyle& style) {
    const LLFontGL* font = style.fontSize() > 0.f ? fontForStyle(style) : nullptr;
    if (style.fontSize() <= 0.f || text.empty())
        return {0.f, lineHeight(style, font)};
    if (!font)
        return {0.f, lineHeight(style)};
    const LLWString wide = utf8str_to_wstring(text);
    const float width = font->getWidthF32(wide.c_str(), 0, S32_MAX, true, usedTextSpacing(style));
    return {std::ceil(std::max(0.f, width)), lineHeight(style, font)};
}

float TextPainter::usedLetterSpacing(const Style::ComputedStyle& style) const {
    if (style.fontSize() <= 0.f)
        return 0.f;
    return style.letterSpacing().resolve(style.fontSize());
}

const LLFontGL* TextPainter::fontForStyle(const Style::ComputedStyle& style) {
    const std::uint64_t currentGeneration = geometry.system.generation();
    const S32 currentResolutionGeneration = LLFontGL::sResolutionGeneration;
    if (resourceGeneration != currentGeneration || resolutionGeneration != currentResolutionGeneration) {
        LL_DEBUGS("UI") << "Invalidating font cache for System generation " << currentGeneration << " (previous resource generation "
                        << resourceGeneration << ") at resolution generation " << currentResolutionGeneration
                        << " (previous resolution generation " << resolutionGeneration << ")" << LL_ENDL;
        clearFonts();
        resourceGeneration = currentGeneration;
        resolutionGeneration = currentResolutionGeneration;
    }

    Style::FontSelectionRequest request {style.fontWeight(), style.fontWidth(), style.fontStyle()};
    const float pixelSize = style.fontSize();
    const Style::FontFamilies& familyList = style.fontFamily();
    const Style::FontFamilies noFamilyList;
    const LLFontGL* primaryFont = nullptr;
    bool firstFamily = true;
    for (const Style::FontFamily& family : familyList) {
        const bool isNextFamily = !firstFamily;
        firstFamily = false;
        const bool isFallback = primaryFont != nullptr;
        const Style::FontFamilies& cascade = isFallback ? noFamilyList : familyList;
        const LLFontGL* font = nullptr;
        if (const auto* name = std::get_if<std::string>(&family)) {
            const std::vector<const CSS::FontFace*> candidates = CSS::fontFacesInMatchOrder(geometry.system.fontFaces(), *name, request);
            for (const CSS::FontFace* candidate : candidates) {
                if (candidate->origin != CSS::StyleOrigin::Skin)
                    continue;
                if (const LLFontGL* candidateFont = fontForFace(*candidate, pixelSize, request, isFallback, cascade)) {
                    font = candidateFont;
                    break;
                }
            }
            if (!font && !candidates.empty())
                continue;
        }

        if (!font) {
            if (const auto source = systemFont(family, request)) {
                const std::string selectedFamily = describeFontFamily(family);
                font = fontForSource(*source, pixelSize, request, isFallback, cascade, selectedFamily, "system", source->familyName,
                    isNextFamily ? "next family" : "installed family");
            }
        }
        if (!font)
            continue;
        if (!primaryFont)
            primaryFont = font;
        else
            primaryFont->addFallbackFont(*font);
    }

    const Style::FontFamily fallbackFamily {Style::GenericFontFamily::SansSerif};
    const auto fallback = systemFont(fallbackFamily, request);
    if (!fallback)
        return primaryFont;
    const Style::FontFamilies& cascade = primaryFont ? noFamilyList : familyList;
    const LLFontGL* fallbackFont = fontForSource(*fallback, pixelSize, request, primaryFont != nullptr, cascade,
        describeFontFamily(fallbackFamily), "generic", "sans-serif", "final generic");
    if (!fallbackFont)
        return primaryFont;
    if (!primaryFont)
        return fallbackFont;
    primaryFont->addFallbackFont(*fallbackFont);
    return primaryFont;
}

const LLFontGL* TextPainter::fontForFace(const CSS::FontFace& face, float pixelSize, const Style::FontSelectionRequest& request,
    bool isFallback, const Style::FontFamilies& familyList) {
    if (!std::isfinite(pixelSize) || pixelSize <= 0.f)
        return nullptr;
    for (const CSS::FontFaceSource& source : face.sources) {
        if (const auto* local = std::get_if<CSS::FontFaceLocal>(&source.value)) {
            if (const auto match = localFont(local->name)) {
                if (const LLFontGL* font = fontForSource(*match, pixelSize, request, isFallback, familyList, face.family, "local",
                        local->name, isFallback ? "next family" : "authored @font-face")) {
                    return font;
                }
            }
            continue;
        }

        const CSS::FontFaceURL& url = std::get<CSS::FontFaceURL>(source.value);
        if (!url.id.valid() || !gFontManagerp)
            continue;
        const auto* bytes = geometry.system.resourceData(url.id.value());
        if (!bytes || bytes->empty())
            continue;
        auto [fontKey, inserted] = memoryFontKeys.try_emplace(url.id.value());
        if (inserted)
            fontKey->second = gFontManagerp->registerFontBytes(source.sourceName, *bytes);
        if (fontKey->second.empty())
            continue;
        if (const LLFontGL* font = fontForSource(detail::SystemFontMatch {fontKey->second}, pixelSize, request, isFallback, familyList,
                face.family, "resource", url.url, isFallback ? "next family" : "authored @font-face")) {
            return font;
        }
    }
    return nullptr;
}

const LLFontGL* TextPainter::fontForSource(const detail::SystemFontMatch& source, float pixelSize,
    const Style::FontSelectionRequest& request, bool isFallback, const Style::FontFamilies& familyList, std::string_view selectedFamily,
    std::string_view sourceKind, std::string_view sourceName, std::string_view reason) {
    if (source.path.empty() || !gFontManagerp || !std::isfinite(pixelSize) || pixelSize <= 0.f)
        return nullptr;
    const FontInstanceKey key {source.path, source.faceIndex, pixelSize, request.weight.value, request.width.percentage, request.style,
        familyList, isFallback};
    auto [cached, inserted] = fontInstances.try_emplace(key, nullptr);
    if (!inserted)
        return cached->second.get();

    ALFontVarAxes axes;
    axes.wght = std::clamp(request.weight.value, 1.f, 1000.f);
    axes.wght_set = true;
    axes.wdth = std::clamp(request.width.percentage, 50.f, 200.f);
    axes.wdth_set = true;
    if (request.style == Style::FontStyle::Italic) {
        axes.ital = 1.f;
        axes.ital_set = true;
    } else if (request.style == Style::FontStyle::Oblique) {
        axes.slnt = -14.f;
        axes.slnt_set = true;
    }

    auto font = std::make_unique<LLFontGL>();
    constexpr float pointsPerCssPixel = 72.f / 96.f;
    constexpr float cssDPI = 96.f;
    if (!font->loadFace(source.path, pixelSize * pointsPerCssPixel, cssDPI, cssDPI, isFallback, source.faceIndex, EFontHinting::DEFAULT,
            LLFontGL::NORMAL, axes)) {
        font->destroyGL();
        return nullptr;
    }
    cached->second = std::move(font);
    const std::string_view selectedFace = source.fullName.empty() ? selectedFamily : source.fullName;
    LL_DEBUGS("UI") << fmt::format("Resolved UI font: requested-css-families=[{}], css-family={}, selected-face={} ({}, {}), source={}:{} "
                                   "[{}#{}], role={}, reason={}, "
                                   "System generation={}, resource generation={}",
        describeFontFamilies(familyList), selectedFamily, selectedFace, source.familyName, source.postScriptName, sourceKind, sourceName,
        source.path, source.faceIndex, isFallback ? "fallback" : "primary", reason, geometry.system.generation(), resourceGeneration)
                    << LL_ENDL;
    return cached->second.get();
}

std::optional<detail::SystemFontMatch> TextPainter::systemFont(const Style::FontFamily& family,
    const Style::FontSelectionRequest& request) {
    const auto key = std::tuple {family, request.weight.value, request.width.percentage, request.style};
    auto [match, inserted] = systemFontMatches.try_emplace(key);
    if (inserted)
        match->second = detail::matchSystemFont(family, request);
    return match->second;
}

std::optional<detail::SystemFontMatch> TextPainter::localFont(std::string_view name) {
    const std::string key(name);
    auto [match, inserted] = localFontMatches.try_emplace(key);
    if (inserted)
        match->second = detail::matchLocalFont(name);
    return match->second;
}

float TextPainter::lineHeight(const Style::ComputedStyle& style, const LLFontGL* font) {
    const auto& lineHeightValue = style.lineHeight().mValue;
    if (const auto* length = std::get_if<Style::LineHeight::Length>(&lineHeightValue))
        return std::ceil(length->pixels);
    if (const auto* number = std::get_if<Style::LineHeight::Number>(&lineHeightValue))
        return std::ceil(style.fontSize() * number->value);
    if (style.fontSize() <= 0.f)
        return 0.f;
    if (!font)
        font = fontForStyle(style);
    return font ? static_cast<float>(font->getLineHeight()) : 0.f;
}

void TextPainter::clearFonts() {
    for (auto& [key, font] : fontInstances) {
        (void)key;
        if (font)
            font->destroyGL();
    }
    fontInstances.clear();
    memoryFontKeys.clear();
    localFontMatches.clear();
    systemFontMatches.clear();
    if (gFontManagerp)
        gFontManagerp->collectGarbage();
}

void TextPainter::destroyGL() {
    clearFonts();
    resourceGeneration = geometry.system.generation();
    resolutionGeneration = LLFontGL::sResolutionGeneration;
}

void OpenGLPaintContext::Impl::beginFrame(const PaintTarget& target) {
    mFrameNativeAppearance = target.nativeAppearance;
    effects.resetFrame();
    clipStack.beginFrame(target);
    geometry.beginFrame(target);
    colorMask.emplace(true, true);
    uiState = std::make_unique<LLGLSUIDefault>();
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
}

void OpenGLPaintContext::Impl::endFrame() {
    if (effects.depth() != 0) {
        llassert(false);
        effects.resetFrame();
    }
    clipStack.popAllTranslations();
    clipStack.popAll();
    if (geometry.program.mProgramObject) {
        geometry.program.bind();
        geometry.applyClipCoverage(geometry.program);
    }
    gUIProgram.bind();
    geometry.applyClipCoverage(gUIProgram);
    uiState.reset();
    colorMask.reset();
    mFrameNativeAppearance = nullptr;
}

Layout::Vec2 OpenGLPaintContext::Impl::measureText(const std::string& textValue, const Style::ComputedStyle& style) {
    return text.measureText(textValue, style);
}

float OpenGLPaintContext::Impl::usedLetterSpacing(const Style::ComputedStyle& style) const { return text.usedLetterSpacing(style); }

void OpenGLPaintContext::Impl::pushClip(const Layout::Rect& rect, float scale, Layout::ClipAxes axes) { clipStack.push(rect, scale, axes); }

void OpenGLPaintContext::Impl::popClip() { clipStack.pop(); }

void OpenGLPaintContext::Impl::pushTranslation(const Layout::Vec2& translation) { clipStack.pushTranslation(translation); }

void OpenGLPaintContext::Impl::popTranslation() { clipStack.popTranslation(); }

void OpenGLPaintContext::Impl::beginEffects(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale) {
    effects.begin(rect, style, scale);
}

void OpenGLPaintContext::Impl::endEffects() { effects.end(); }

void OpenGLPaintContext::Impl::paintNativeScrollbar(const NativeScrollbarPaintRequest& request) {
    const NativeAppearance& appearance = nativeAppearance();
    const std::optional<ResolvedScrollbarClip> scrollbarClip = resolveScrollbarClip(request.clip);
    const ResolvedScrollbarClip* clip = scrollbarClip ? &*scrollbarClip : nullptr;
    const auto paint = [this, clip](const Layout::Rect& rect, Color color, float radius) {
        if (rect.empty() || color.a <= 0.f)
            return;
        const Layout::Rect box = snapped(rect);
        geometry.drawRoundedShape(PaintOp::Fill, box, resolveBorderRadius(box, Style::BorderRadius::uniform(Style::Length {radius})), 0.f,
            color, Style::OutlineStyle::Solid, std::nullopt, nullptr, clip);
    };
    const auto paintArrow = [this, clip](const Layout::Rect& rect, ScrollbarAxis axis, bool pointsPositive, Color color) {
        geometry.drawArrow(rect, axis, pointsPositive, color, clip);
    };
    const auto paintAxis = [&](const NativeScrollbarAxisGeometry& axis) {
        if (!axis.visible)
            return;
        const NativeScrollbarPaintStyle style = appearance.scrollbarPaintStyle(request, axis.axis);
        paint(axis.bounds, style.track, 0.f);
        paintArrow(snappedScrollbarArrow(axis, true), axis.axis, axis.axis == ScrollbarAxis::Vertical || axis.reversed, style.startArrow);
        paintArrow(snappedScrollbarArrow(axis, false), axis.axis, axis.axis == ScrollbarAxis::Horizontal && !axis.reversed, style.endArrow);
        paint(axis.thumb, style.thumb, style.thumbRadius);
    };
    paintAxis(request.geometry.horizontal);
    paintAxis(request.geometry.vertical);
    if (request.geometry.hasCorner) {
        const ScrollbarAxis axis = request.geometry.horizontal.visible ? ScrollbarAxis::Horizontal : ScrollbarAxis::Vertical;
        paint(request.geometry.corner, appearance.scrollbarPaintStyle(request, axis).track, 0.f);
    }
}

void OpenGLPaintContext::Impl::paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style,
    std::optional<TopBorderGap> topBorderGap, const BackgroundPaintContext* backgroundContext) {
    geometry.paintBox(rect, style, topBorderGap, backgroundContext);
}

void OpenGLPaintContext::Impl::paintText(const std::string& textValue, const Layout::Rect& rect, const Style::ComputedStyle& style) {
    text.paintText(textValue, rect, style);
}

void OpenGLPaintContext::Impl::paintNativeInputMark(const NativeInputMarkPaintRequest& request) { geometry.drawNativeInputMark(request); }

bool EffectRenderer::captureFramebuffer(const Layout::Rect& capture, float scale, LLRenderTarget& target) {
    const U32 width = static_cast<U32>(std::max(1.f, std::round(capture.w * scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(capture.h * scale)));
    if (!ensureTarget(target, width, height))
        return false;

    const detail::PaintState state = mClips.snapshot();
    Layout::Rect sourceCapture = capture;
    if (state.target.kind == PaintTargetKind::Direct) {
        const Layout::Vec2 translation = mClips.translation();
        sourceCapture.x += translation.x;
        sourceCapture.y += translation.y;
    }
    const S32 sourceX = ll_round(state.target.pixelOrigin.x + (sourceCapture.x - state.origin.x) * scale);
    const S32 sourceY = ll_round(state.target.pixelOrigin.y + (sourceCapture.y - state.origin.y) * scale);
    gGL.flush();
    target.bindTexture(0, 0, ALSamplers::BilinearClamp);
    glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, sourceX, sourceY, static_cast<GLsizei>(width), static_cast<GLsizei>(height));
    return true;
}

LLRenderTarget* EffectRenderer::applyBlur(LLRenderTarget& source, LLRenderTarget& horizontalTarget, LLRenderTarget& verticalTarget,
    const Layout::Rect& capture, const Layout::Rect& effectRect, const Style::FilterOperation& operation, float scale) {
    if (!mProgram.mProgramObject)
        return &source;
    const U32 width = source.getWidth();
    const U32 height = source.getHeight();
    if (!ensureTarget(horizontalTarget, width, height) || !ensureTarget(verticalTarget, width, height))
        return &source;
    const std::optional<BlurProfile> profile = resolveBlurProfile(operation);
    if (!profile)
        return &source;

    constexpr float kRadiansPerDegree = std::numbers::pi_v<float> / 180.f;
    const float angle = profile->angleDegrees * kRadiansPerDegree;
    const Layout::Vec2 direction(std::sin(angle), std::cos(angle));
    const float extent = (std::abs(direction.x) * effectRect.w + std::abs(direction.y) * effectRect.h) * scale;
    const Layout::Vec2 center((effectRect.x + effectRect.w * .5f - capture.x) * scale,
        (effectRect.y + effectRect.h * .5f - capture.y) * scale);
    const Layout::Vec2 gradientLineStart = center - direction * (extent * .5f);
    const Layout::Vec2 gradientLine = direction * extent;
    const Layout::Vec2 gradientStart = gradientLineStart + gradientLine * profile->start.position;
    Layout::Vec2 gradientEnd = gradientLineStart + gradientLine * profile->end.position;
    if (profile->start.position == profile->end.position)
        gradientEnd = gradientStart + direction * std::max(1.f, scale);
    const float maximumRadius = static_cast<float>(std::max(width, height));

    const PaintShaderUniforms& uniforms = shaderUniforms();

    auto pass = [&](LLRenderTarget& input, LLRenderTarget& output, float axisX, float axisY) {
        LLGLDisable disableScissor(GL_SCISSOR_TEST);
        LLGLDisable disableBlend(GL_BLEND);
        gGL.flush();
        detail::RenderTargetGuard targetGuard(output);
        detail::MatrixGuard matrixGuard({0.f, 0.f, capture.w, capture.h}, scale);
        mProgram.bind();
        setPaintOp(mProgram, PaintOp::Blur);
        setClipCoverageUniforms(mProgram, std::nullopt);
        mProgram.uniform2f(uniforms.effectTextureSize, static_cast<float>(width), static_cast<float>(height));
        mProgram.uniform2f(uniforms.effectBlurAxis, axisX, axisY);
        mProgram.uniform2f(uniforms.effectBlurRadii, std::min(profile->start.stdDeviation * scale, maximumRadius),
            std::min(profile->end.stdDeviation * scale, maximumRadius));
        mProgram.uniform2f(uniforms.effectGradientStart, gradientStart.x, gradientStart.y);
        mProgram.uniform2f(uniforms.effectGradientEnd, gradientEnd.x, gradientEnd.y);
        mProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &input, ALSamplers::BilinearClamp);
        drawTexturedQuad({0.f, 0.f, capture.w, capture.h});
        gGL.flush();
        mProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
        setPaintOp(mProgram, PaintOp::Direct);
    };

    pass(source, horizontalTarget, 1.f, 0.f);
    pass(horizontalTarget, verticalTarget, 0.f, 1.f);
    mClips.reapply();
    return &verticalTarget;
}

void EffectRenderer::compositeEffect(LLRenderTarget& source, const Layout::Rect& capture, const Layout::Rect& destination,
    const ResolvedBorderRadius& radii, bool roundedMask) {
    const Layout::Rect visible = Layout::intersectRects(capture, destination);
    if (!mProgram.mProgramObject || visible.empty() || capture.empty())
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();

    const float u0 = (visible.left() - capture.left()) / capture.w;
    const float u1 = (visible.right() - capture.left()) / capture.w;
    const float v0 = (visible.bottom() - capture.bottom()) / capture.h;
    const float v1 = (visible.top() - capture.bottom()) / capture.h;
    mProgram.bind();
    setPaintOp(mProgram, PaintOp::Composite);
    setClipCoverageUniforms(mProgram, mClips.coverageBounds());
    mProgram.uniform4f(uniforms.effectCaptureRect, capture.x, capture.y, capture.w, capture.h);
    mProgram.uniform4f(uniforms.effectMaskRect, destination.x, destination.y, destination.w, destination.h);
    mProgram.uniform4f(uniforms.effectMaskRadiusX, radii.topLeft.x, radii.topRight.x, radii.bottomRight.x, radii.bottomLeft.x);
    mProgram.uniform4f(uniforms.effectMaskRadiusY, radii.topLeft.y, radii.topRight.y, radii.bottomRight.y, radii.bottomLeft.y);
    mProgram.uniform1i(uniforms.effectRoundedMask, roundedMask ? 1 : 0);
    mProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &source, ALSamplers::BilinearClamp);
    drawTexturedQuad(visible, u0, v0, u1, v1);
    gGL.flush();
    mProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
    setPaintOp(mProgram, PaintOp::Direct);
}

void EffectRenderer::compositeMaskedEffect(LLRenderTarget& source, LLRenderTarget& mask, const Layout::Rect& capture) {
    if (!mProgram.mProgramObject || capture.empty())
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    mProgram.bind();
    setPaintOp(mProgram, PaintOp::CompositeMask);
    setClipCoverageUniforms(mProgram, mClips.coverageBounds());
    mProgram.uniform1i(uniforms.maskMode, 0);
    mProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &source, ALSamplers::BilinearClamp);
    mProgram.bindTexture(LLShaderMgr::ALTERNATE_DIFFUSE_MAP, &mask, ALSamplers::BilinearClamp);
    drawTexturedQuad(capture);
    gGL.flush();
    mProgram.unbindTexture(LLShaderMgr::ALTERNATE_DIFFUSE_MAP);
    mProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
    mProgram.uniform1i(uniforms.maskMode, 0);
    setPaintOp(mProgram, PaintOp::Direct);
}

void EffectRenderer::begin(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale) {
    if (mEffectDepth == mEffectLayers.size())
        mEffectLayers.emplace_back();
    EffectLayer& frame = mEffectLayers[mEffectDepth++];
    frame.filter.operations.clear();
    frame.maskLayers.clear();
    frame.hasMask = mGeometry.hasRenderableMask(style.maskLayers);
    if (frame.hasMask) {
        frame.maskLayers = style.maskLayers;
        frame.maskStyle = style;
    }
    frame.effectRect = rect;
    frame.scale = std::max(scale, .0001f);
    frame.capture.reset();
    const float maximumPadding = std::max(mClips.bounds().w, mClips.bounds().h);

    auto captureBounds = [&](float padding) {
        const Layout::Rect expanded = snappedOutward(expandedRect(rect, padding), frame.scale);
        Layout::Rect visible = mClips.bounds();
        if (mClips.targetKind() == PaintTargetKind::Direct) {
            const Layout::Vec2 translation = mClips.translation();
            visible = {visible.x - translation.x, visible.y - translation.y, visible.w, visible.h};
        }
        return Layout::intersectRects(expanded, visible);
    };

    if (!style.backdropFilter().isNone()) {
        float padding = 1.f / frame.scale;
        bool hasBlur = false;
        for (const Style::FilterOperation& operation : style.backdropFilter().operations) {
            const std::optional<float> stdDeviation = maximumBlurDeviation(operation);
            if (!stdDeviation || *stdDeviation <= 0.f)
                continue;
            hasBlur = true;
            padding = std::min(padding + *stdDeviation * 2.f, maximumPadding);
        }
        if (hasBlur) {
            const Layout::Rect capture = captureBounds(padding);
            if (!capture.empty() && captureFramebuffer(capture, frame.scale, mBackgroundTargets[0])) {
                LLRenderTarget* source = &mBackgroundTargets[0];
                for (const Style::FilterOperation& operation : style.backdropFilter().operations) {
                    const std::optional<float> stdDeviation = maximumBlurDeviation(operation);
                    if (!stdDeviation || *stdDeviation <= 0.f)
                        continue;
                    LLRenderTarget& horizontal = mBackgroundTargets[1];
                    LLRenderTarget& vertical = source == &mBackgroundTargets[0] ? mBackgroundTargets[2] : mBackgroundTargets[0];
                    source = applyBlur(*source, horizontal, vertical, capture, rect, operation, frame.scale);
                }
                compositeEffect(*source, capture, rect, resolveBorderRadius(rect, style.borderRadius()), true);
            }
        }
    }

    frame.filter = style.filter;
    if (frame.filter.isNone() && !frame.hasMask)
        return;
    float padding = 1.f / frame.scale;
    for (const Style::FilterOperation& operation : frame.filter.operations) {
        const std::optional<float> stdDeviation = maximumBlurDeviation(operation);
        if (stdDeviation)
            padding = std::min(padding + *stdDeviation * 2.f, maximumPadding);
    }
    frame.captureRect = captureBounds(padding);
    if (frame.captureRect.empty())
        return;

    const U32 width = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.w * frame.scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.h * frame.scale)));
    if (!ensureTarget(frame.targets[0], width, height))
        return;
    gGL.flush();
    frame.capture.emplace(mClips, frame.targets[0], frame.captureRect, frame.scale);
    mClips.reapply();
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
}

void EffectRenderer::end() {
    if (mEffectDepth == 0) {
        llassert(false);
        return;
    }
    EffectLayer& frame = mEffectLayers[--mEffectDepth];
    if (!frame.capture)
        return;

    gGL.flush();
    frame.capture.reset();
    mClips.reapply();

    LLRenderTarget* source = &frame.targets[0];
    for (const Style::FilterOperation& operation : frame.filter.operations) {
        LLRenderTarget& horizontal = frame.targets[1];
        LLRenderTarget& vertical = source == &frame.targets[0] ? frame.targets[2] : frame.targets[0];
        source = applyBlur(*source, horizontal, vertical, frame.captureRect, frame.effectRect, operation, frame.scale);
    }
    if (!frame.hasMask) {
        compositeEffect(*source, frame.captureRect, frame.captureRect, uniformBorderRadius(0.f), false);
        return;
    }

    const U32 width = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.w * frame.scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.h * frame.scale)));
    if (!ensureTarget(frame.maskTarget, width, height)) {
        compositeEffect(*source, frame.captureRect, frame.captureRect, uniformBorderRadius(0.f), false);
        return;
    }
    {
        gGL.flush();
        detail::EffectCaptureGuard maskCapture(mClips, frame.maskTarget, frame.captureRect, frame.scale);
        gGL.blendFunc(LLRender::BF_ONE, LLRender::BF_ZERO, LLRender::BF_ONE, LLRender::BF_ZERO);
        mGeometry.paintMaskLayers(frame.effectRect, frame.maskStyle, frame.maskLayers);
    }
    mClips.reapply();
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
    compositeMaskedEffect(*source, frame.maskTarget, frame.captureRect);
}

void GeometryPainter::prepareVectorDraw() {
    gGL.getTextureSlot(0)->unbind();
    if (program.mProgramObject) {
        program.bind();
        setPaintOp(program, PaintOp::Direct);
        applyClipCoverage(program);
    }
}

void TextPainter::prepareTextDraw() { gUIProgram.bind(); }

void GeometryPainter::drawMesh(const Mesh& mesh) {
    if (mesh.empty() || !program.mProgramObject)
        return;
    prepareVectorDraw();
    gGL.begin(LLRender::TRIANGLES);
    for (const Vertex& vertex : mesh.vertices) {
        gGL.color4f(vertex.color.r, vertex.color.g, vertex.color.b, vertex.color.a);
        gGL.vertex2f(vertex.position.x, vertex.position.y);
    }
    gGL.end();
}

void GeometryPainter::drawGradientMesh(const Mesh& mesh, const Layout::Rect& rect, const Style::Gradient& gradient) {
    if (mesh.empty() || !program.mProgramObject || rect.empty())
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, PaintOp::GradientMesh);
    program.uniform4f(uniforms.shapeRect, rect.x, rect.y, rect.w, rect.h);
    program.uniform2f(uniforms.shapeOffset, 0.f, 0.f);
    setGradientUniforms(program, uniforms, rect, gradient);
    gGL.begin(LLRender::TRIANGLES);
    for (const Vertex& vertex : mesh.vertices) {
        gGL.color4f(vertex.color.r, vertex.color.g, vertex.color.b, vertex.color.a);
        gGL.texCoord2f(vertex.position.x - rect.x, vertex.position.y - rect.y);
        gGL.vertex2f(vertex.position.x, vertex.position.y);
    }
    gGL.end();
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawBorderImageGradientTile(const Layout::Rect& tile, const Layout::Rect& source, float imageWidth, float imageHeight,
    const Style::Gradient& gradient, float opacity) {
    if (!program.mProgramObject || tile.empty() || source.empty())
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, PaintOp::Gradient);
    program.uniform4f(uniforms.shapeRect, tile.x, tile.y, tile.w, tile.h);
    setBorderRadiusUniforms(program, uniforms, uniformBorderRadius(0.f), false);
    setBorderRadiusUniforms(program, uniforms, uniformBorderRadius(0.f), true);
    program.uniform2f(uniforms.shapeOffset, 0.f, 0.f);
    setScrollbarClipUniforms(program, uniforms, tile, nullptr);
    setGradientUniforms(program, uniforms, {0.f, 0.f, imageWidth, imageHeight}, gradient, source.w / tile.w, source.h / tile.h, source.x,
        source.y);
    drawShapeQuad(tile, opacity);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::setRoundedClip(const ResolvedBorderRadius& radii) {
    if (!program.mProgramObject)
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const Layout::Rect clip = clips.pixelRect();
    const float scale = clips.pixelScale();
    program.bind();
    program.uniform1i(uniforms.roundedClipEnabled, 1);
    program.uniform4f(uniforms.roundedClipRect, clip.x, clip.y, clip.w, clip.h);
    program.uniform4f(uniforms.roundedClipRadiusX, radii.topLeft.x * scale, radii.topRight.x * scale, radii.bottomRight.x * scale,
        radii.bottomLeft.x * scale);
    program.uniform4f(uniforms.roundedClipRadiusY, radii.topLeft.y * scale, radii.topRight.y * scale, radii.bottomRight.y * scale,
        radii.bottomLeft.y * scale);
}

void GeometryPainter::clearRoundedClip() {
    if (!program.mProgramObject)
        return;
    program.bind();
    program.uniform1i(shaderUniforms().roundedClipEnabled, 0);
}

void GeometryPainter::drawArrow(const Layout::Rect& rect, ScrollbarAxis axis, bool pointsPositive, const Color& color,
    const ResolvedScrollbarClip* clip) {
    const Layout::Rect box = snapped(rect);
    if (!program.mProgramObject || box.empty() || color.a <= 0.f)
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const GLint direction = axis == ScrollbarAxis::Horizontal ? (pointsPositive ? 1 : 0) : (pointsPositive ? 3 : 2);
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, PaintOp::Arrow);
    program.uniform4f(uniforms.shapeRect, box.x, box.y, box.w, box.h);
    program.uniform4f(uniforms.shapeColor, color.r, color.g, color.b, color.a);
    program.uniform2f(uniforms.shapeOffset, 0.f, 0.f);
    program.uniform1i(uniforms.arrowDirection, direction);
    setScrollbarClipUniforms(program, uniforms, box, clip);
    drawShapeQuad(box);
    gGL.flush();
    program.uniform1i(uniforms.scrollbarClipEnabled, 0);
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawNativeInputMark(const NativeInputMarkPaintRequest& request) {
    const Layout::Rect bounds = snapped(request.bounds);
    if (!program.mProgramObject || bounds.empty() || request.color.a <= 0.f)
        return;

    if (request.mark == NativeInputMark::Dash) {
        const float radius = std::min(request.radius, std::min(bounds.w, bounds.h) * .5f);
        drawRoundedShape(PaintOp::Fill, bounds, uniformBorderRadius(radius), 0.f, request.color);
        return;
    }

    if (request.strokeWidth <= 0.f)
        return;
    if (request.path.empty())
        return;
    drawMesh(
        tessellateStroke(request.path, request.color, request.strokeWidth, coverageFringeWidth(request.scale), Style::StrokeCap::Butt));
}

void TextPainter::paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style) {
    if (text.empty() || style.fontSize() <= 0.f || style.color().resolvedColor().a <= 0.f)
        return;
    const LLFontGL* resolvedFont = fontForStyle(style);
    if (!resolvedFont)
        return;
    const LLFontGL& font = *resolvedFont;
    prepareTextDraw();
    geometry.applyClipCoverage(gUIProgram);
    const LLVector3 uiTranslation = gGL.getUITranslation();
    const Layout::Rect glyphRect {rect.x + uiTranslation.mV[VX], rect.y + uiTranslation.mV[VY], rect.w, rect.h};
    const LLFontGL::HAlign horizontal = horizontalAlignment(style);
    constexpr LLFontGL::VAlign vertical = LLFontGL::VCENTER;
    const Color& styleColor = style.color().resolvedColor();
    const LLColor4 color(styleColor.r, styleColor.g, styleColor.b, styleColor.a);
    const LLFontGL::TextSpacing spacing = usedTextSpacing(style);
    U8 fontStyle = style.fontWeight().value >= 600.f ? LLFontGL::BOLD : LLFontGL::NORMAL;
    if (style.fontStyle() != Style::FontStyle::Normal)
        fontStyle |= LLFontGL::ITALIC;
    if (hasTextDecoration(style.textDecoration(), Style::TextDecoration::Underline))
        fontStyle |= LLFontGL::UNDERLINE;
    font.renderUTF8(text, 0, textX(glyphRect, horizontal), textY(glyphRect, vertical), color, horizontal, vertical, fontStyle,
        LLFontGL::NO_SHADOW, S32_MAX, S32_MAX, nullptr, false, true, spacing);
    if (hasTextDecoration(style.textDecoration(), Style::TextDecoration::LineThrough)) {
        const float width = measureText(text, style).x;
        const float anchor = textX(rect, horizontal);
        const float left = horizontal == LLFontGL::RIGHT ? anchor - width : horizontal == LLFontGL::HCENTER ? anchor - width * .5f : anchor;
        const float thickness = std::max(1.f, std::round(font.getLineHeight() / 14.f));
        const float y = textBaseline(rect, vertical, font) + font.getAscenderHeight() * .3f;
        geometry.drawRoundedShape(PaintOp::Fill, {left, y - thickness * .5f, width, thickness}, uniformBorderRadius(0.f), 0.f,
            style.color().resolvedColor());
    }
}

void GeometryPainter::drawRoundedShape(PaintOp op, const Layout::Rect& rect, const ResolvedBorderRadius& radii, float borderWidth,
    const Color& color, Style::OutlineStyle outlineStyle, std::optional<TopBorderGap> topBorderGap, const ResolvedBorderRadius* innerRadii,
    const ResolvedScrollbarClip* clip) {
    if (!program.mProgramObject || rect.empty() || color.a <= 0.f || (op == PaintOp::Border && borderWidth <= 0.f))
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const ResolvedBorderRadius& inner = innerRadii ? *innerRadii : radii;
    const float padding = coverageFringe();
    const Layout::Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, op);
    program.uniform4f(uniforms.shapeRect, rect.x, rect.y, rect.w, rect.h);
    setBorderRadiusUniforms(program, uniforms, radii, false);
    setBorderRadiusUniforms(program, uniforms, inner, true);
    program.uniform1f(uniforms.shapeBorderWidth, std::clamp(borderWidth, 0.f, std::min(rect.w, rect.h) * 0.5f));
    program.uniform4f(uniforms.shapeColor, color.r, color.g, color.b, color.a);
    program.uniform2f(uniforms.shapeOffset, padding, padding);
    program.uniform1i(uniforms.outlineStyle, outlineOpValue(outlineStyle));
    program.uniform2f(uniforms.topBorderGap, topBorderGap ? topBorderGap->left - rect.left() : -1.f,
        topBorderGap ? topBorderGap->right - rect.left() : -1.f);
    setScrollbarClipUniforms(program, uniforms, rect, clip);
    drawShapeQuad(quad);
    gGL.flush();
    program.uniform1i(uniforms.scrollbarClipEnabled, 0);
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawRoundedGradient(const Layout::Rect& rect, const ResolvedBorderRadius& radii, const Style::Gradient& gradient,
    const Layout::RectEdges<float>* borderWidths, std::optional<TopBorderGap> topBorderGap, const ResolvedBorderRadius* innerRadii) {
    if (!program.mProgramObject || rect.empty() || gradient.stops.size() < 2 || gradient.stops.size() > 8
        || (borderWidths && !borderWidths->any()))
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const ResolvedBorderRadius& inner = innerRadii ? *innerRadii : radii;

    const float padding = coverageFringe();
    const Layout::Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, borderWidths ? PaintOp::GradientBorder : PaintOp::Gradient);
    program.uniform4f(uniforms.shapeRect, rect.x, rect.y, rect.w, rect.h);
    setBorderRadiusUniforms(program, uniforms, radii, false);
    setBorderRadiusUniforms(program, uniforms, inner, true);
    program.uniform2f(uniforms.shapeOffset, padding, padding);
    if (borderWidths)
        program.uniform4f(uniforms.borderWidths, borderWidths->top, borderWidths->right, borderWidths->bottom, borderWidths->left);
    program.uniform2f(uniforms.topBorderGap, topBorderGap ? topBorderGap->left - rect.left() : -1.f,
        topBorderGap ? topBorderGap->right - rect.left() : -1.f);
    setGradientUniforms(program, uniforms, rect, gradient);
    drawShapeQuad(quad);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::paintImageLayers(const Layout::Rect& rect, const Style::ComputedStyle& style,
    std::span<const Style::BackgroundLayer> layers, const Color& vectorColor, const Style::Gradient* vectorGradient,
    const BackgroundPaintContext* backgroundContext) {
    for (auto layer = layers.rbegin(); layer != layers.rend(); ++layer) {
        Layout::Rect origin = backgroundBox(rect, style, layer->origin);
        Layout::Rect clip = backgroundBox(rect, style, layer->clip);
        if (backgroundContext) {
            if (layer->attachment == Style::BackgroundAttachment::Fixed && !backgroundContext->viewport.empty()) {
                origin = backgroundContext->viewport;
                origin.x -= backgroundContext->paintTranslation.x;
                origin.y -= backgroundContext->paintTranslation.y;
            } else if (layer->attachment == Style::BackgroundAttachment::Local) {
                origin.x += backgroundContext->localScrollTranslation.x;
                origin.y += backgroundContext->localScrollTranslation.y;
                if (backgroundContext->scrollport)
                    clip = Layout::intersectRects(clip, *backgroundContext->scrollport);
            }
        }
        if (origin.empty() || clip.empty())
            continue;

        const SVGImage* svg = resourceSvg(*layer);
        const RasterImage* raster = resourceRaster(*layer);
        if (!layer->image.gradient() && !svg && !raster)
            continue;

        const Layout::Rect source = svg ? svg->viewBox
            : raster                    ? Layout::Rect {0.f, 0.f, static_cast<float>(raster->width), static_cast<float>(raster->height)}
                                        : Layout::Rect {0.f, 0.f, origin.w, origin.h};
        const float sourceWidth = std::max(.0001f, source.w);
        const float sourceHeight = std::max(.0001f, source.h);
        float imageWidth = sourceWidth;
        float imageHeight = sourceHeight;
        if (layer->size.mode == Style::BackgroundSizeType::Cover || layer->size.mode == Style::BackgroundSizeType::Contain) {
            const float widthScale = origin.w / sourceWidth;
            const float heightScale = origin.h / sourceHeight;
            const float scale = layer->size.mode == Style::BackgroundSizeType::Cover ? std::max(widthScale, heightScale)
                                                                                     : std::min(widthScale, heightScale);
            imageWidth *= scale;
            imageHeight *= scale;
        } else if (layer->size.mode == Style::BackgroundSizeType::Explicit) {
            if (layer->size.width)
                imageWidth = std::max(0.f, layer->size.width->resolve(origin.w));
            if (layer->size.height)
                imageHeight = std::max(0.f, layer->size.height->resolve(origin.h));
            if (!layer->size.width && layer->size.height)
                imageWidth = imageHeight * sourceWidth / sourceHeight;
            else if (layer->size.width && !layer->size.height)
                imageHeight = imageWidth * sourceHeight / sourceWidth;
        }
        if (imageWidth <= 0.f || imageHeight <= 0.f)
            continue;

        const Layout::Rect image {origin.x + layer->position.x.resolve(origin.w - imageWidth),
            origin.y + layer->position.y.resolve(origin.h - imageHeight), imageWidth, imageHeight};
        const bool repeatX = layer->repeat == Style::BackgroundRepeat::Repeat || layer->repeat == Style::BackgroundRepeat::RepeatX;
        const bool repeatY = layer->repeat == Style::BackgroundRepeat::Repeat || layer->repeat == Style::BackgroundRepeat::RepeatY;
        const int firstX = repeatX ? static_cast<int>(std::floor((clip.left() - image.left()) / image.w)) : 0;
        const int lastX = repeatX ? static_cast<int>(std::ceil((clip.right() - image.left()) / image.w)) : 0;
        const int firstY = repeatY ? static_cast<int>(std::floor((clip.bottom() - image.bottom()) / image.h)) : 0;
        const int lastY = repeatY ? static_cast<int>(std::ceil((clip.top() - image.bottom()) / image.h)) : 0;

        clips.push(clip, mTargetScale, Layout::ClipAxes::Both);
        setRoundedClip(normalizeBorderRadius(backgroundBoxRadii(rect, style, layer->clip), clip.w, clip.h));
        if (const Style::Gradient* gradient = layer->image.gradient()) {
            for (int y = firstY; y <= lastY; ++y)
                for (int x = firstX; x <= lastX; ++x)
                    drawRoundedGradient(
                        {image.x + image.w * static_cast<float>(x), image.y + image.h * static_cast<float>(y), image.w, image.h},
                        uniformBorderRadius(0.f), *gradient);
            clearRoundedClip();
            clips.pop();
            continue;
        }

        if (raster) {
            LLGLTexture* texture = rasterTexture(*layer->image.resource());
            if (texture) {
                const PaintShaderUniforms& uniforms = shaderUniforms();
                for (int y = firstY; y <= lastY; ++y)
                    for (int x = firstX; x <= lastX; ++x) {
                        const Layout::Rect tile {image.x + image.w * static_cast<float>(x), image.y + image.h * static_cast<float>(y),
                            image.w, image.h};
                        prepareVectorDraw();
                        program.bind();
                        setPaintOp(program, PaintOp::Image);
                        program.uniform4f(uniforms.shapeRect, tile.x, tile.y, tile.w, tile.h);
                        program.bindTexture(LLShaderMgr::DIFFUSE_MAP, texture, ALSamplers::BilinearClamp);
                        drawShapeQuad(tile, style.opacity().value);
                        gGL.flush();
                        program.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
                        setPaintOp(program, PaintOp::Direct);
                    }
            }
            clearRoundedClip();
            clips.pop();
            continue;
        }

        const Color color = vectorGradient ? Color(1.f, 1.f, 1.f, 1.f) : vectorColor;
        const float unitScale = std::min(image.w / sourceWidth, image.h / sourceHeight);
        const float strokeWidth = style.svgStrokeWidth ? style.svgStrokeWidth->pixels : svg->strokeWidth * unitScale;
        const Style::StrokeCap cap = style.svgStrokeCapSet ? style.svgStrokeCap : svg->strokeCap;
        for (int y = firstY; y <= lastY; ++y)
            for (int x = firstX; x <= lastX; ++x) {
                const Layout::Rect tile {image.x + image.w * static_cast<float>(x), image.y + image.h * static_cast<float>(y), image.w,
                    image.h};
                for (const Path& path : svg->paths) {
                    const Mesh mesh = tessellateStroke(transformSVGPath(path, source, tile), color, strokeWidth, coverageFringe(), cap);
                    if (vectorGradient)
                        drawGradientMesh(mesh, tile, *vectorGradient);
                    else
                        drawMesh(mesh);
                }
            }
        clearRoundedClip();
        clips.pop();
    }
}

const SVGImage* GeometryPainter::resourceSvg(const Style::BackgroundLayer& layer) const {
    const std::string* resource = layer.image.resource();
    if (!resource)
        return nullptr;
    const SVGImage* image = system.resourceSvg(*resource);
    return image && !image->empty() ? image : nullptr;
}

const RasterImage* GeometryPainter::resourceRaster(const Style::BackgroundLayer& layer) const {
    const std::string* resource = layer.image.resource();
    if (!resource)
        return nullptr;
    const RasterImage* image = system.resourceRaster(*resource);
    return image && !image->empty() ? image : nullptr;
}

LLGLTexture* GeometryPainter::rasterTexture(std::string_view reference) {
    const std::string key(reference);
    const auto found = rasterTextures.find(key);
    if (found != rasterTextures.end())
        return found->second.get();

    const RasterImage* image = system.resourceRaster(reference);
    if (!image || image->empty() || image->width > std::numeric_limits<U16>::max() || image->height > std::numeric_limits<U16>::max())
        return nullptr;
    LLPointer<LLImageRaw> raw = new LLImageRaw(static_cast<U16>(image->width), static_cast<U16>(image->height), 4);
    std::copy(image->rgba.begin(), image->rgba.end(), raw->getData());
    LLPointer<LLGLTexture> texture = new LLGLTexture(raw.get(), false);
    if (texture->hasGLTexture()) {
        LLGLTexture* result = texture.get();
        rasterTextures.emplace(key, std::move(texture));
        return result;
    }
    return nullptr;
}

bool GeometryPainter::hasRenderableMask(std::span<const Style::MaskLayer> layers) const {
    return std::any_of(layers.begin(), layers.end(), [this](const Style::MaskLayer& layer) {
        if (layer.image.image.gradient())
            return true;
        return resourceSvg(layer.image) != nullptr || resourceRaster(layer.image) != nullptr;
    });
}

void GeometryPainter::paintMaskLayers(const Layout::Rect& rect, const Style::ComputedStyle& style,
    std::span<const Style::MaskLayer> layers) {
    if (!program.mProgramObject || layers.empty())
        return;
    Style::ComputedStyle maskStyle = style;
    maskStyle.setColor(Color(1.f, 1.f, 1.f, 1.f));
    maskStyle.setOpacity(Style::Opacity {1.f});
    for (auto layer = layers.rbegin(); layer != layers.rend(); ++layer) {
        program.bind();
        program.uniform1i(shaderUniforms().maskMode,
            layer->mode == Style::MaskMode::Alpha || (layer->mode == Style::MaskMode::MatchSource && layer->type == Style::MaskType::Alpha)
                ? 0
                : 1);
        const Style::MaskComposite composite = layer == layers.rbegin() ? Style::MaskComposite::Add : layer->composite;
        switch (composite) {
        case Style::MaskComposite::Add:
            gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE,
                LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
            break;
        case Style::MaskComposite::Subtract:
            gGL.blendFunc(LLRender::BF_ONE_MINUS_DEST_ALPHA, LLRender::BF_ZERO, LLRender::BF_ONE_MINUS_DEST_ALPHA, LLRender::BF_ZERO);
            break;
        case Style::MaskComposite::Intersect:
            gGL.blendFunc(LLRender::BF_DEST_ALPHA, LLRender::BF_ZERO, LLRender::BF_DEST_ALPHA, LLRender::BF_ZERO);
            break;
        case Style::MaskComposite::Exclude:
            gGL.blendFunc(LLRender::BF_ONE_MINUS_DEST_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_DEST_ALPHA,
                LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
            break;
        }
        paintImageLayers(rect, maskStyle, std::span<const Style::BackgroundLayer>(&layer->image, 1), maskStyle.color().resolvedColor(),
            nullptr);
    }
    program.bind();
    program.uniform1i(shaderUniforms().maskMode, 0);
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
}

void GeometryPainter::drawShadow(const Layout::Rect& rect, const ResolvedBorderRadius& radii, const Style::BoxShadow& shadow) {
    if (!program.mProgramObject || rect.empty() || shadow.color.resolvedColor().a <= 0.f)
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();

    const Layout::Rect box = rect;
    Layout::Rect shape = box;
    Layout::Rect quad = box;
    Layout::Vec2 localShapeOffset;
    ResolvedBorderRadius shapeRadii = radii;
    ResolvedBorderRadius innerRadii = radii;
    PaintOp op = PaintOp::InsetShadow;
    if (!shadow.inset) {
        op = PaintOp::OuterShadow;
        shape = {box.x + shadow.horizontal - shadow.spread, box.y - shadow.vertical - shadow.spread,
            std::max(0.f, box.w + shadow.spread * 2.f), std::max(0.f, box.h + shadow.spread * 2.f)};
        if (shape.empty())
            return;
        shapeRadii = expandedBorderRadius(radii, shadow.spread, shape.w, shape.h);
        innerRadii = shapeRadii;
        const float padding = shadow.blur * 2.f + coverageFringe();
        quad = {shape.x - padding, shape.y - padding, shape.w + padding * 2.f, shape.h + padding * 2.f};
        localShapeOffset = {padding, padding};
    } else {
        const Layout::Rect hole = {0.f, 0.f, std::max(0.f, box.w - shadow.spread * 2.f), std::max(0.f, box.h - shadow.spread * 2.f)};
        innerRadii = insetBorderRadius(radii, {shadow.spread, shadow.spread, shadow.spread, shadow.spread}, hole.w, hole.h);
        const float padding = coverageFringe();
        quad = expandedRect(box, padding);
        localShapeOffset = {padding, padding};
    }

    prepareVectorDraw();
    program.bind();
    setPaintOp(program, op);
    program.uniform4f(uniforms.shapeRect, shape.x, shape.y, shape.w, shape.h);
    setBorderRadiusUniforms(program, uniforms, shapeRadii, false);
    setBorderRadiusUniforms(program, uniforms, innerRadii, true);
    const Color& color = shadow.color.resolvedColor();
    program.uniform4f(uniforms.shapeColor, color.r, color.g, color.b, color.a);
    program.uniform2f(uniforms.shapeOffset, localShapeOffset.x, localShapeOffset.y);
    program.uniform2f(uniforms.shadowOffset, shadow.horizontal, -shadow.vertical);
    program.uniform1f(uniforms.shadowBlur, shadow.blur);
    program.uniform1f(uniforms.shadowSpread, shadow.spread);
    drawShapeQuad(quad);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawShapeQuad(const Layout::Rect& rect, float alpha) {
    gGL.begin(LLRender::TRIANGLES);
    gGL.color4f(1.f, 1.f, 1.f, alpha);
    gGL.texCoord2f(0.f, 0.f);
    gGL.vertex2f(rect.left(), rect.bottom());
    gGL.texCoord2f(rect.w, 0.f);
    gGL.vertex2f(rect.right(), rect.bottom());
    gGL.texCoord2f(rect.w, rect.h);
    gGL.vertex2f(rect.right(), rect.top());
    gGL.texCoord2f(0.f, 0.f);
    gGL.vertex2f(rect.left(), rect.bottom());
    gGL.texCoord2f(rect.w, rect.h);
    gGL.vertex2f(rect.right(), rect.top());
    gGL.texCoord2f(0.f, rect.h);
    gGL.vertex2f(rect.left(), rect.top());
    gGL.end();
}

void GeometryPainter::drawBorder(const Layout::Rect& rect, const Style::ComputedStyle& style, std::optional<TopBorderGap> topBorderGap) {
    const Layout::RectEdges<float> width = Layout::borderWidths(style);
    if (!width.any())
        return;

    const Layout::RectEdges<Style::BorderStyle> borderStyles = style.borderStyle();
    const Layout::RectEdges<Style::Color> borderColors = style.borderColor();
    const auto sideColor = [](Style::BorderStyle borderStyle, const Style::Color& styleColor, bool leadingSide) {
        const Color& color = styleColor.resolvedColor();
        if (borderStyle == Style::BorderStyle::Solid || borderStyle == Style::BorderStyle::NoneValue)
            return color;
        const bool highlight = (borderStyle == Style::BorderStyle::Outset) == leadingSide;
        const Color tint = highlight ? Color(1.f, 1.f, 1.f, color.a) : Color(0.f, 0.f, 0.f, color.a);
        return shade(color, tint, .45f);
    };
    const bool uniformPaint = width.isUniform() && borderStyles.isUniform() && borderColors.isUniform();
    const Layout::Rect box = rect;
    const ResolvedBorderRadius borderRadii = resolveBorderRadius(box, style.borderRadius());
    const bool square = borderRadii.topLeft.x == 0.f && borderRadii.topLeft.y == 0.f && borderRadii.topRight.x == 0.f
        && borderRadii.topRight.y == 0.f && borderRadii.bottomRight.x == 0.f && borderRadii.bottomRight.y == 0.f
        && borderRadii.bottomLeft.x == 0.f && borderRadii.bottomLeft.y == 0.f;
    if (uniformPaint) {
        const Style::BorderStyle borderStyle = borderStyles.top;
        const Color& borderColor = borderColors.top.resolvedColor();
        if (borderStyle == Style::BorderStyle::NoneValue || borderColor.a <= 0.f)
            return;

        if (borderStyle != Style::BorderStyle::Solid && square && (!topBorderGap || topBorderGap->empty())) {
            const float uniformWidth = width.top;
            const Color topLeft = sideColor(borderStyle, borderColors.top, true);
            const Color bottomRight = sideColor(borderStyle, borderColors.bottom, false);
            const ResolvedBorderRadius zeroRadii = uniformBorderRadius(0.f);
            drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - uniformWidth, box.w, uniformWidth}, zeroRadii, 0.f, topLeft);
            drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom(), box.w, uniformWidth}, zeroRadii, 0.f, bottomRight);
            const float height = std::max(0.f, box.h - uniformWidth * 2.f);
            drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom() + uniformWidth, uniformWidth, height}, zeroRadii, 0.f, topLeft);
            drawRoundedShape(PaintOp::Fill, {box.right() - uniformWidth, box.bottom() + uniformWidth, uniformWidth, height}, zeroRadii, 0.f,
                bottomRight);
            return;
        }

        const Layout::Rect innerBox = Layout::insetRect(box, width);
        const ResolvedBorderRadius innerRadii = insetBorderRadius(borderRadii, width, innerBox.w, innerBox.h);
        drawRoundedShape(PaintOp::Border, box, borderRadii, width.top, borderColor, Style::OutlineStyle::Solid, topBorderGap, &innerRadii);
        return;
    }

    const Color topColor = sideColor(borderStyles.top, borderColors.top, true);
    const Color rightColor = sideColor(borderStyles.right, borderColors.right, false);
    const Color bottomColor = sideColor(borderStyles.bottom, borderColors.bottom, false);
    const Color leftColor = sideColor(borderStyles.left, borderColors.left, true);
    const ResolvedBorderRadius zeroRadii = uniformBorderRadius(0.f);
    if (topBorderGap && !topBorderGap->empty()) {
        const float gapLeft = std::clamp(topBorderGap->left, box.left(), box.right());
        const float gapRight = std::clamp(topBorderGap->right, gapLeft, box.right());
        drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width.top, std::max(0.f, gapLeft - box.left()), width.top}, zeroRadii, 0.f,
            topColor);
        drawRoundedShape(PaintOp::Fill, {gapRight, box.top() - width.top, std::max(0.f, box.right() - gapRight), width.top}, zeroRadii, 0.f,
            topColor);
    } else
        drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width.top, box.w, width.top}, zeroRadii, 0.f, topColor);
    drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom(), box.w, width.bottom}, zeroRadii, 0.f, bottomColor);
    drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom() + width.bottom, width.left, box.h - width.top - width.bottom}, zeroRadii, 0.f,
        leftColor);
    drawRoundedShape(PaintOp::Fill, {box.right() - width.right, box.bottom() + width.bottom, width.right, box.h - width.top - width.bottom},
        zeroRadii, 0.f, rightColor);
}

bool GeometryPainter::drawBorderImage(const Layout::Rect& rect, const Style::ComputedStyle& style) {
    const Style::Image& source = style.borderImageSource();
    const std::string* resource = source.resource();
    const RasterImage* image = resource ? system.resourceRaster(*resource) : nullptr;
    if (image && image->empty())
        image = nullptr;
    const SVGImage* svg = resource ? system.resourceSvg(*resource) : nullptr;
    if (svg && svg->empty())
        svg = nullptr;
    const Style::Gradient* gradient = source.gradient();
    if (!image && !svg && !gradient)
        return false;

    const Layout::RectEdges<Style::LineWidth> computedWidths = style.borderWidth();
    const Layout::RectEdges<float> widths {computedWidths.top.pixels, computedWidths.right.pixels, computedWidths.bottom.pixels,
        computedWidths.left.pixels};
    const auto area = resolveBorderImageArea(rect, style.borderImageOutset(), widths);
    if (!area)
        return false;
    const float imageWidth = image ? static_cast<float>(image->width) : svg ? svg->viewBox.w : area->w;
    const float imageHeight = image ? static_cast<float>(image->height) : svg ? svg->viewBox.h : area->h;
    const auto grid = resolveBorderImageGrid(style.borderImageSlice(), style.borderImageWidth(), style.borderImageOutset(),
        style.borderImageRepeat(), rect, imageWidth, imageHeight, widths);
    if (!grid)
        return false;

    LLGLTexture* texture = image && resource ? rasterTexture(*resource) : nullptr;
    if (image && !texture)
        return false;
    if (!image) {
        const Color& vectorColor = style.color().resolvedColor();
        for (const Layout::BorderImagePatch& patch : grid->pieces) {
            if (patch.source.empty() || patch.destination.empty())
                continue;
            const auto horizontal =
                Layout::borderImageTilePlan(patch.repeatX, patch.destination.left(), patch.destination.w, patch.tileSize.x);
            const auto vertical =
                Layout::borderImageTilePlan(patch.repeatY, patch.destination.bottom(), patch.destination.h, patch.tileSize.y);
            if (!horizontal || !vertical)
                continue;

            clips.push(patch.destination, mTargetScale, Layout::ClipAxes::Both);
            for (std::size_t y = 0; y < vertical->count; ++y) {
                for (std::size_t x = 0; x < horizontal->count; ++x) {
                    const Layout::Rect tile {horizontal->position(x), vertical->position(y), horizontal->size, vertical->size};
                    if (gradient) {
                        drawBorderImageGradientTile(tile, patch.source, imageWidth, imageHeight, *gradient, style.opacity().value);
                        continue;
                    }

                    const float scale = std::min(tile.w / patch.source.w, tile.h / patch.source.h);
                    const float strokeWidth = style.svgStrokeWidth ? style.svgStrokeWidth->pixels : svg->strokeWidth * scale;
                    const Style::StrokeCap cap = style.svgStrokeCapSet ? style.svgStrokeCap : svg->strokeCap;
                    const Layout::Rect svgTarget {tile.x - patch.source.x * tile.w / patch.source.w,
                        tile.y - patch.source.y * tile.h / patch.source.h, imageWidth * tile.w / patch.source.w,
                        imageHeight * tile.h / patch.source.h};
                    for (const Path& path : svg->paths) {
                        const Mesh mesh = tessellateStroke(transformSVGPath(path, svg->viewBox, svgTarget), vectorColor, strokeWidth,
                            coverageFringe(), cap);
                        drawMesh(mesh);
                    }
                }
            }
            clips.pop();
        }
        return true;
    }

    prepareVectorDraw();
    program.bind();
    program.bindTexture(LLShaderMgr::DIFFUSE_MAP, texture, ALSamplers::BilinearClamp);
    setPaintOp(program, PaintOp::Image);

    for (const Layout::BorderImagePatch& patch : grid->pieces) {
        if (patch.source.empty() || patch.destination.empty())
            continue;
        const auto horizontal = Layout::borderImageTilePlan(patch.repeatX, patch.destination.left(), patch.destination.w, patch.tileSize.x);
        const auto vertical = Layout::borderImageTilePlan(patch.repeatY, patch.destination.bottom(), patch.destination.h, patch.tileSize.y);
        if (!horizontal || !vertical)
            continue;

        const float insetX = std::min(.5f, patch.source.w * .5f) / static_cast<float>(image->width);
        const float insetY = std::min(.5f, patch.source.h * .5f) / static_cast<float>(image->height);
        const float leftU = (patch.source.left() + insetX * static_cast<float>(image->width)) / static_cast<float>(image->width);
        const float rightU = (patch.source.right() - insetX * static_cast<float>(image->width)) / static_cast<float>(image->width);
        const float bottomV = (patch.source.bottom() + insetY * static_cast<float>(image->height)) / static_cast<float>(image->height);
        const float topV = (patch.source.top() - insetY * static_cast<float>(image->height)) / static_cast<float>(image->height);
        const Layout::Rect& area = patch.destination;
        const PaintShaderUniforms& uniforms = shaderUniforms();
        program.uniform4f(uniforms.shapeRect, area.x, area.y, area.w, area.h);
        clips.push(area, mTargetScale, Layout::ClipAxes::Both);
        gGL.begin(LLRender::TRIANGLES);
        const auto vertex = [&](float x, float y, float u, float v) {
            gGL.color4f(1.f, 1.f, 1.f, style.opacity().value);
            gGL.texCoord2f(u * area.w, v * area.h);
            gGL.vertex2f(x, y);
        };
        for (std::size_t y = 0; y < vertical->count; ++y) {
            for (std::size_t x = 0; x < horizontal->count; ++x) {
                const Layout::Rect tile {horizontal->position(x), vertical->position(y), horizontal->size, vertical->size};
                vertex(tile.left(), tile.bottom(), leftU, bottomV);
                vertex(tile.right(), tile.bottom(), rightU, bottomV);
                vertex(tile.right(), tile.top(), rightU, topV);
                vertex(tile.left(), tile.bottom(), leftU, bottomV);
                vertex(tile.right(), tile.top(), rightU, topV);
                vertex(tile.left(), tile.top(), leftU, topV);
            }
        }
        gGL.end();
        gGL.flush();
        clips.pop();
    }

    program.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
    setPaintOp(program, PaintOp::Direct);
    return true;
}

void GeometryPainter::drawOutline(const Layout::Rect& rect, const Style::ComputedStyle& style) {
    if (style.outline().width <= 0.f || style.outline().color.resolvedColor().a <= 0.f)
        return;
    const float width = style.outline().width;
    const float expansion = width + style.outline().offset.pixels;
    const Layout::Rect box = rect;
    const Layout::Rect outlineBox = {box.x - expansion, box.y - expansion, box.w + expansion * 2.f, box.h + expansion * 2.f};
    const ResolvedBorderRadius outlineRadii =
        expandedBorderRadius(resolveBorderRadius(box, style.borderRadius()), expansion, outlineBox.w, outlineBox.h);
    const Layout::Rect innerBox = Layout::insetRect(outlineBox, {width, width, width, width});
    const ResolvedBorderRadius innerRadii = insetBorderRadius(outlineRadii, {width, width, width, width}, innerBox.w, innerBox.h);
    drawRoundedShape(PaintOp::Border, outlineBox, outlineRadii, width, style.outline().color.resolvedColor(), style.outline().style,
        std::nullopt, &innerRadii);
}

void GeometryPainter::paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style, std::optional<TopBorderGap> topBorderGap,
    const BackgroundPaintContext* backgroundContext) {
    const Layout::Rect box = rect;
    const ResolvedBorderRadius borderRadii = resolveBorderRadius(box, style.borderRadius());
    const Style::BoxShadows& shadows = style.boxShadow();
    for (auto shadow = shadows.rbegin(); shadow != shadows.rend(); ++shadow)
        if (!shadow->inset)
            drawShadow(rect, borderRadii, *shadow);

    const bool bordered = hasVisibleBorder(style);
    const Style::BackgroundBox backgroundClip =
        style.backgroundLayers.empty() ? Style::BackgroundBox::BorderBox : style.backgroundLayers.back().clip;
    const Layout::Rect fillBox = backgroundBox(box, style, backgroundClip);
    const ResolvedBorderRadius fillRadii = backgroundBoxRadii(box, style, backgroundClip);
    if (style.backgroundColor().resolvedColor().a > 0.f)
        drawRoundedShape(PaintOp::Fill, fillBox, fillRadii, 0.f, style.backgroundColor().resolvedColor());
    const auto* strokeColor = std::get_if<Style::Color>(&style.stroke);
    const auto* strokeImage = std::get_if<Style::Image>(&style.stroke);
    const Color transparent(0.f, 0.f, 0.f, 0.f);
    paintImageLayers(box, style, style.backgroundLayers, strokeColor ? strokeColor->resolvedColor() : transparent,
        strokeImage ? strokeImage->gradient() : nullptr, backgroundContext);
    if (!drawBorderImage(rect, style) && bordered)
        drawBorder(rect, style, topBorderGap);
    const Layout::RectEdges<float> borderInsets = Layout::borderWidths(style);
    const Layout::Rect insetBox = Layout::insetRect(box, borderInsets);
    const ResolvedBorderRadius insetRadii = insetBorderRadius(borderRadii, borderInsets, insetBox.w, insetBox.h);
    for (auto shadow = shadows.rbegin(); shadow != shadows.rend(); ++shadow)
        if (shadow->inset)
            drawShadow(insetBox, insetRadii, *shadow);
    drawOutline(rect, style);
}
} // namespace Core
