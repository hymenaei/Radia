/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "render/openglpaintcontext.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <deque>
#include <numbers>
#include <optional>
#include <utility>
#include <vector>
#include "llfontgl.h"
#include "llgl.h"
#include "llglslshader.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llshadermgr.h"
#include "llstring.h"
#include "render/openglpaintstate.h"
#include "render/svg.h"
#include "render/tessellator.h"
#include "system.h"
#include "v4color.h"

namespace radia::ui {
namespace {
Rect snapped(const Rect& rect) {
    const float left = std::round(rect.left());
    const float right = std::round(rect.right());
    const float bottom = std::round(rect.bottom());
    const float top = std::round(rect.top());
    return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

Rect snappedScrollbarArrow(const ScrollbarAxisGeometry& axis, bool start) {
    const Rect arrow = start ? axis.startArrow : axis.endArrow;
    if (!axis.visible || arrow.empty()) return {};

    const Rect bounds = snapped(axis.bounds);
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

bool hasVisibleBorder(const ComputedStyle& style) {
    return style.borderWidth.any() && (style.borderGradient.has_value() || style.borderColor.a > 0.f);
}

bool hasOpaqueBackground(const ComputedStyle& style) {
    if (!style.backgroundGradient) return style.backgroundColor.a >= 1.f;
    return std::all_of(style.backgroundGradient->stops.begin(), style.backgroundGradient->stops.end(),
                       [](const GradientStop& stop) { return stop.color.a >= 1.f; });
}

Color shade(Color source, Color target, float amount) {
    return {source.r + (target.r - source.r) * amount, source.g + (target.g - source.g) * amount, source.b + (target.b - source.b) * amount,
            source.a};
}

struct ResolvedBorderRadii {
    Vec2 topLeft;
    Vec2 topRight;
    Vec2 bottomRight;
    Vec2 bottomLeft;
};

Vec2 resolveCornerRadius(const BorderRadius& radius, float width, float height) {
    return {std::max(0.f, radius.horizontal.resolve(width)), std::max(0.f, radius.vertical.resolve(height))};
}

ResolvedBorderRadii normalizeBorderRadii(ResolvedBorderRadii radii, float width, float height) {
    float scale = 1.f;
    const auto limit = [&scale](float available, float sum) {
        if (sum > 0.f) scale = std::min(scale, std::max(0.f, available) / sum);
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

ResolvedBorderRadii resolveBorderRadii(const Rect& rect, const BorderRadii& source) {
    const float width = std::max(0.f, rect.w);
    const float height = std::max(0.f, rect.h);
    return normalizeBorderRadii({resolveCornerRadius(source.topLeft, width, height), resolveCornerRadius(source.topRight, width, height),
                                 resolveCornerRadius(source.bottomRight, width, height), resolveCornerRadius(source.bottomLeft, width, height)},
                                width, height);
}

ResolvedBorderRadii insetBorderRadii(const ResolvedBorderRadii& radii, const EdgeInsets& inset, float width, float height) {
    return normalizeBorderRadii(
        {
            {std::max(0.f, radii.topLeft.x - inset.left), std::max(0.f, radii.topLeft.y - inset.top)},
            {std::max(0.f, radii.topRight.x - inset.right), std::max(0.f, radii.topRight.y - inset.top)},
            {std::max(0.f, radii.bottomRight.x - inset.right), std::max(0.f, radii.bottomRight.y - inset.bottom)},
            {std::max(0.f, radii.bottomLeft.x - inset.left), std::max(0.f, radii.bottomLeft.y - inset.bottom)},
        },
        width, height);
}

ResolvedBorderRadii expandedBorderRadii(const ResolvedBorderRadii& radii, float amount, float width, float height) {
    return normalizeBorderRadii({radii.topLeft + Vec2(amount, amount), radii.topRight + Vec2(amount, amount),
                                 radii.bottomRight + Vec2(amount, amount), radii.bottomLeft + Vec2(amount, amount)},
                                width, height);
}

ResolvedBorderRadii uniformBorderRadii(float radius) {
    const Vec2 corner(std::max(0.f, radius), std::max(0.f, radius));
    return {corner, corner, corner, corner};
}

struct ResolvedScrollbarClip {
    Rect rect;
    ResolvedBorderRadii radii;
};

std::optional<ResolvedScrollbarClip> resolveScrollbarClip(const NativeScrollbarClip& source) {
    if (!source.enabled || source.borderBox.empty()) return std::nullopt;
    const Rect innerBox = insetRect(source.borderBox, source.borderWidth);
    if (innerBox.empty()) return std::nullopt;
    const ResolvedBorderRadii outerRadii = resolveBorderRadii(source.borderBox, source.borderRadius);
    return ResolvedScrollbarClip{innerBox, insetBorderRadii(outerRadii, source.borderWidth, innerBox.w, innerBox.h)};
}

Rect expandedRect(const Rect& rect, float amount) {
    return {rect.x - amount, rect.y - amount, rect.w + amount * 2.f, rect.h + amount * 2.f};
}

float coverageFringeWidth(float scale) {
    return scale > .0001f ? 1.f / scale : 1.f;
}

Rect snappedOutward(const Rect& rect, float scale) {
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
    if (!target.isComplete()) return target.allocate(width, height, GL_RGBA8);
    if (target.getWidth() != width || target.getHeight() != height) target.resize(width, height);
    return target.isComplete() && target.getWidth() == width && target.getHeight() == height;
}

LLFontGL::HAlign horizontalAlignment(const ComputedStyle& style) {
    return style.textAlign == TextAlign::Center ? LLFontGL::HCENTER : style.textAlign == TextAlign::Right ? LLFontGL::RIGHT : LLFontGL::LEFT;
}

float textX(const Rect& rect, LLFontGL::HAlign align) {
    return align == LLFontGL::HCENTER ? rect.x + rect.w * 0.5f : align == LLFontGL::RIGHT ? rect.right() : rect.x;
}

float textY(const Rect& rect, LLFontGL::VAlign align) {
    return align == LLFontGL::VCENTER ? rect.y + rect.h * 0.5f : align == LLFontGL::BOTTOM ? rect.y : rect.top();
}

float textBaseline(const Rect& rect, LLFontGL::VAlign align, const LLFontGL& font) {
    const float anchor = textY(rect, align);
    if (align == LLFontGL::TOP) return anchor - font.getAscenderHeight();
    if (align == LLFontGL::BOTTOM) return anchor + font.getDescenderHeight();
    if (align == LLFontGL::VCENTER) return anchor - (font.getAscenderHeight() - font.getDescenderHeight()) * .5f;
    return anchor;
}

const LLFontGL& fontForStyle(const ComputedStyle& style) {
    LLFontGL* font = LLFontGL::getFontAtPixelSize("SansSerif", style.fontSize, style.fontWeight, style.fontItalic);
    if (!font) LL_ERRS("UI") << "OpenGL text adapter used before viewer fonts were initialized." << LL_ENDL;
    return *font;
}

float textLineHeight(const ComputedStyle& style) {
    if (style.lineHeight) return std::ceil(style.lineHeight->pixels);
    if (style.fontSize <= 0.f) return 0.f;
    return static_cast<float>(fontForStyle(style).getLineHeight());
}

LLFontGL::TextSpacing usedTextSpacing(const ComputedStyle& style, const LLFontGL& font) {
    static const LLWString sSpace(U" ");
    const float spaceAdvance = std::max(0.f, font.getWidthF32(sSpace.c_str(), 0, 1, true));
    return {
        style.letterSpacing.resolve(spaceAdvance),
        style.wordSpacing.resolve(style.fontSize),
    };
}

Vec2 measureOpenGLText(const std::string& text, const ComputedStyle& style) {
    if (style.fontSize <= 0.f) return {0.f, textLineHeight(style)};
    if (text.empty()) return {0.f, textLineHeight(style)};
    const LLWString wide = utf8str_to_wstring(text);
    const LLFontGL& font = fontForStyle(style);
    const LLFontGL::TextSpacing spacing = usedTextSpacing(style, font);
    const float width = font.getWidthF32(wide.c_str(), 0, S32_MAX, true, spacing);
    return {std::ceil(std::max(0.f, width)), textLineHeight(style)};
}

void drawTexturedQuad(const Rect& rect, float u0 = 0.f, float v0 = 0.f, float u1 = 1.f, float v1 = 1.f) {
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
#include "render/paintprotocol.def"
#undef PAINT_OP_ENTRY
#undef GRADIENT_OP_ENTRY
#undef OUTLINE_OP_ENTRY
};

enum class GradientOp : GLint {
#define PAINT_OP_ENTRY(name, value)
#define GRADIENT_OP_ENTRY(name, value) name = value,
#define OUTLINE_OP_ENTRY(name, value)
#include "render/paintprotocol.def"
#undef PAINT_OP_ENTRY
#undef GRADIENT_OP_ENTRY
#undef OUTLINE_OP_ENTRY
};

enum class OutlineOp : GLint {
#define PAINT_OP_ENTRY(name, value)
#define GRADIENT_OP_ENTRY(name, value)
#define OUTLINE_OP_ENTRY(name, value) name = value,
#include "render/paintprotocol.def"
#undef PAINT_OP_ENTRY
#undef GRADIENT_OP_ENTRY
#undef OUTLINE_OP_ENTRY
};

struct PaintShaderUniforms {
    LLStaticHashedString paintOp{"paintOp"};
    LLStaticHashedString shapeRect{"shapeRect"};
    LLStaticHashedString shapeRadiusX{"shapeRadiusX"};
    LLStaticHashedString shapeRadiusY{"shapeRadiusY"};
    LLStaticHashedString innerRadiusX{"innerRadiusX"};
    LLStaticHashedString innerRadiusY{"innerRadiusY"};
    LLStaticHashedString scrollbarClipRect{"scrollbarClipRect"};
    LLStaticHashedString scrollbarClipRadiusX{"scrollbarClipRadiusX"};
    LLStaticHashedString scrollbarClipRadiusY{"scrollbarClipRadiusY"};
    LLStaticHashedString scrollbarClipEnabled{"scrollbarClipEnabled"};
    LLStaticHashedString shapeBorderWidth{"shapeBorderWidth"};
    LLStaticHashedString shapeColor{"shapeColor"};
    LLStaticHashedString shapeOffset{"shapeOffset"};
    LLStaticHashedString arrowDirection{"arrowDirection"};
    LLStaticHashedString outlineStyle{"outlineStyle"};
    LLStaticHashedString borderWidths{"borderWidths"};
    LLStaticHashedString topBorderGap{"topBorderGap"};
    LLStaticHashedString gradientKind{"gradientKind"};
    LLStaticHashedString gradientRepeating{"gradientRepeating"};
    LLStaticHashedString gradientStart{"gradientStart"};
    LLStaticHashedString gradientEnd{"gradientEnd"};
    LLStaticHashedString gradientCenter{"gradientCenter"};
    LLStaticHashedString gradientRadius{"gradientRadius"};
    LLStaticHashedString gradientAngle{"gradientAngle"};
    LLStaticHashedString gradientStopCount{"gradientStopCount"};
    LLStaticHashedString gradientColors{"gradientColors"};
    LLStaticHashedString gradientStops{"gradientStops"};
    LLStaticHashedString shadowOffset{"shadowOffset"};
    LLStaticHashedString shadowBlur{"shadowBlur"};
    LLStaticHashedString shadowSpread{"shadowSpread"};
    LLStaticHashedString effectTextureSize{"effectTextureSize"};
    LLStaticHashedString effectBlurAxis{"effectBlurAxis"};
    LLStaticHashedString effectBlurRadii{"effectBlurRadii"};
    LLStaticHashedString effectGradientStart{"effectGradientStart"};
    LLStaticHashedString effectGradientEnd{"effectGradientEnd"};
    LLStaticHashedString effectCaptureRect{"effectCaptureRect"};
    LLStaticHashedString effectMaskRect{"effectMaskRect"};
    LLStaticHashedString effectMaskRadiusX{"effectMaskRadiusX"};
    LLStaticHashedString effectMaskRadiusY{"effectMaskRadiusY"};
    LLStaticHashedString effectRoundedMask{"effectRoundedMask"};
    LLStaticHashedString clipCoverageRect{"clipCoverageRect"};
    LLStaticHashedString clipCoverageEnabled{"clipCoverageEnabled"};
};

const PaintShaderUniforms& shaderUniforms() {
    static const PaintShaderUniforms sUniforms;
    return sUniforms;
}

GLint gradientOpValue(GradientKind kind) {
    switch (kind) {
        case GradientKind::Linear: return static_cast<GLint>(GradientOp::Linear);
        case GradientKind::Radial: return static_cast<GLint>(GradientOp::Radial);
        case GradientKind::Conic: return static_cast<GLint>(GradientOp::Conic);
    }
    llassert(false);
    return static_cast<GLint>(GradientOp::Linear);
}

GLint outlineOpValue(OutlineStyle style) {
    switch (style) {
        case OutlineStyle::Solid: return static_cast<GLint>(OutlineOp::Solid);
        case OutlineStyle::Dashed: return static_cast<GLint>(OutlineOp::Dashed);
    }
    llassert(false);
    return static_cast<GLint>(OutlineOp::Solid);
}

void setPaintOp(LLGLSLShader& program, PaintOp op) {
    program.uniform1i(shaderUniforms().paintOp, static_cast<GLint>(op));
}

void setClipCoverageUniforms(LLGLSLShader& program, const std::optional<Rect>& bounds) {
    const PaintShaderUniforms& uniforms = shaderUniforms();
    if (!bounds || bounds->empty()) {
        program.uniform1i(uniforms.clipCoverageEnabled, 0);
        return;
    }
    program.uniform1i(uniforms.clipCoverageEnabled, 1);
    program.uniform4f(uniforms.clipCoverageRect, bounds->x, bounds->y, bounds->w, bounds->h);
}

void setBorderRadiusUniforms(LLGLSLShader& program, const PaintShaderUniforms& uniforms, const ResolvedBorderRadii& radii, bool inner) {
    const LLStaticHashedString& radiusX = inner ? uniforms.innerRadiusX : uniforms.shapeRadiusX;
    const LLStaticHashedString& radiusY = inner ? uniforms.innerRadiusY : uniforms.shapeRadiusY;
    program.uniform4f(radiusX, radii.topLeft.x, radii.topRight.x, radii.bottomRight.x, radii.bottomLeft.x);
    program.uniform4f(radiusY, radii.topLeft.y, radii.topRight.y, radii.bottomRight.y, radii.bottomLeft.y);
}

void setScrollbarClipUniforms(LLGLSLShader& program, const PaintShaderUniforms& uniforms, const Rect& shapeRect, const ResolvedScrollbarClip* clip) {
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
} // namespace

struct GeometryPainter {
    GeometryPainter(::LLGLSLShader& shapeProgram, paint::ClipStack& clipStack) : program(shapeProgram), clips(clipStack) {}

    void beginFrame(const PaintTarget& target) { mTargetScale = target.scale; }
    void drawMesh(const Mesh& mesh);
    void drawArrow(const Rect& rect, ScrollbarAxis axis, bool pointsPositive, const Color& color, const ResolvedScrollbarClip* clip = nullptr);
    void drawNativeInputMark(const NativeInputMarkPaintRequest& request);
    void drawRoundedShape(PaintOp op, const Rect& rect, const ResolvedBorderRadii& radii, float borderWidth, const Color& color,
                          OutlineStyle outlineStyle = OutlineStyle::Solid, std::optional<TopBorderGap> topBorderGap = std::nullopt,
                          const ResolvedBorderRadii* innerRadii = nullptr, const ResolvedScrollbarClip* clip = nullptr);
    void drawRoundedGradient(const Rect& rect, const ResolvedBorderRadii& radii, const Gradient& gradient, const EdgeInsets* borderWidths = nullptr,
                             std::optional<TopBorderGap> topBorderGap = std::nullopt, const ResolvedBorderRadii* innerRadii = nullptr);
    void drawShadow(const Rect& rect, const ResolvedBorderRadii& radii, const BoxShadow& shadow);
    void drawBorder(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap = std::nullopt);
    void drawOutline(const Rect& rect, const ComputedStyle& style);
    void paintBox(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap);
    void prepareVectorDraw();
    void applyClipCoverage(::LLGLSLShader& target) const { setClipCoverageUniforms(target, clips.coverageBounds()); }
    static void drawShapeQuad(const Rect& rect);
    float coverageFringe() const { return coverageFringeWidth(mTargetScale); }

    ::LLGLSLShader& program;
    paint::ClipStack& clips;
    float mTargetScale = 1.f;
};

struct TextPainter {
    explicit TextPainter(GeometryPainter& geometryPainter) : geometry(geometryPainter) {}

    Vec2 measureText(const std::string& text, const ComputedStyle& style) const;
    float usedLetterSpacing(const ComputedStyle& style) const;
    void paintText(const std::string& text, const Rect& rect, const ComputedStyle& style);
    static void prepareTextDraw();

    GeometryPainter& geometry;
};

struct IconPainter {
    explicit IconPainter(GeometryPainter& geometryPainter) : geometry(geometryPainter) {}

    void paintIcon(const SvgIcon* icon, const Rect& rect, const ComputedStyle& style, float scale);

    GeometryPainter& geometry;
};

class EffectRenderer final {
    struct EffectLayer {
        std::array<LLRenderTarget, 3> targets;
        std::vector<Effect> layerEffects;
        Rect effectRect;
        Rect captureRect;
        float scale = 1.f;
        std::optional<paint::EffectCaptureGuard> capture;
    };

public:
    EffectRenderer(::LLGLSLShader& shapeProgram, paint::ClipStack& clipStack) : mProgram(shapeProgram), mClips(clipStack) {}

    void begin(const Rect& rect, const ComputedStyle& style, float scale);
    void end();
    void resetFrame() {
        while (mEffectDepth > 0) mEffectLayers[--mEffectDepth].capture.reset();
        for (EffectLayer& layer : mEffectLayers) layer.capture.reset();
        mEffectDepth = 0;
    }
    std::size_t depth() const { return mEffectDepth; }

private:
    bool captureFramebuffer(const Rect& capture, float scale, LLRenderTarget& target);
    LLRenderTarget* applyBlur(LLRenderTarget& source, LLRenderTarget& horizontalTarget, LLRenderTarget& verticalTarget, const Rect& capture,
                              const Rect& effectRect, const Effect& effect, float scale);
    void compositeEffect(LLRenderTarget& source, const Rect& capture, const Rect& destination, const ResolvedBorderRadii& radii, bool roundedMask);

    ::LLGLSLShader& mProgram;
    paint::ClipStack& mClips;
    std::array<LLRenderTarget, 3> mBackgroundTargets;
    std::deque<EffectLayer> mEffectLayers;
    std::size_t mEffectDepth = 0;
};

struct OpenGLPaintContext::Impl {
    Impl(::LLGLSLShader& shapeProgram, const System& system)
        : system(system), geometry(shapeProgram, clipStack), text(geometry), icons(geometry), effects(shapeProgram, clipStack) {}

    void beginFrame(const PaintTarget& target);
    void endFrame();
    Vec2 measureText(const std::string& text, const ComputedStyle& style) const;
    float usedLetterSpacing(const ComputedStyle& style) const;
    void pushClip(const Rect& rect, float scale, ClipAxes axes);
    void popClip();
    void pushTranslation(const Vec2& translation);
    void popTranslation();
    void beginEffects(const Rect& rect, const ComputedStyle& style, float scale);
    void endEffects();
    const NativeAppearance& nativeAppearance() const { return mFrameNativeAppearance ? *mFrameNativeAppearance : system.nativeAppearance(); }
    void paintNativeScrollbar(const NativeScrollbarPaintRequest& request);
    void paintNativeInputMark(const NativeInputMarkPaintRequest& request);
    void paintBox(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap);
    void paintText(const std::string& text, const Rect& rect, const ComputedStyle& style);
    void paintIcon(const std::string& name, const Rect& rect, const ComputedStyle& style, float scale);

    std::unique_ptr<LLGLSUIDefault> uiState;
    const System& system;
    const NativeAppearance* mFrameNativeAppearance = nullptr;
    paint::ClipStack clipStack;
    GeometryPainter geometry;
    TextPainter text;
    IconPainter icons;
    EffectRenderer effects;
};

OpenGLPaintContext::OpenGLPaintContext(::LLGLSLShader& shapeProgram, const System& system) : mImpl(std::make_unique<Impl>(shapeProgram, system)) {}

OpenGLPaintContext::~OpenGLPaintContext() = default;

void OpenGLPaintContext::beginFrame(const PaintTarget& target) {
    mImpl->beginFrame(target);
}

void OpenGLPaintContext::endFrame() {
    mImpl->endFrame();
}

Vec2 OpenGLPaintContext::measureText(const std::string& text, const ComputedStyle& style) const {
    return mImpl->measureText(text, style);
}

float OpenGLPaintContext::usedLetterSpacing(const ComputedStyle& style) const {
    return mImpl->usedLetterSpacing(style);
}

std::uint64_t OpenGLPaintContext::generation() const noexcept {
    return mImpl->system.generation();
}

void OpenGLPaintContext::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    mImpl->pushClip(rect, scale, axes);
}

void OpenGLPaintContext::popClip() {
    mImpl->popClip();
}

void OpenGLPaintContext::pushTranslation(const Vec2& translation) {
    mImpl->pushTranslation(translation);
}

void OpenGLPaintContext::popTranslation() {
    mImpl->popTranslation();
}

void OpenGLPaintContext::beginEffects(const Rect& rect, const ComputedStyle& style, float scale) {
    mImpl->beginEffects(rect, style, scale);
}

void OpenGLPaintContext::endEffects() {
    mImpl->endEffects();
}

void OpenGLPaintContext::paintNativeScrollbar(const NativeScrollbarPaintRequest& request) {
    mImpl->paintNativeScrollbar(request);
}

void OpenGLPaintContext::paintNativeInput(const NativeInputPaintRequest& request) {
    mImpl->nativeAppearance().paintInput(*this, request);
}

void OpenGLPaintContext::paintNativeInputMark(const NativeInputMarkPaintRequest& request) {
    mImpl->paintNativeInputMark(request);
}

void OpenGLPaintContext::paintNativeButton(const NativeButtonPaintRequest& request) {
    mImpl->nativeAppearance().paintButton(*this, request);
}

void OpenGLPaintContext::paintBox(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap) {
    mImpl->paintBox(rect, style, topBorderGap);
}

void OpenGLPaintContext::paintText(const std::string& text, const Rect& rect, const ComputedStyle& style) {
    mImpl->paintText(text, rect, style);
}

void OpenGLPaintContext::paintIcon(const std::string& name, const Rect& rect, const ComputedStyle& style, float scale) {
    mImpl->paintIcon(name, rect, style, scale);
}

Vec2 TextPainter::measureText(const std::string& text, const ComputedStyle& style) const {
    return measureOpenGLText(text, style);
}

float TextPainter::usedLetterSpacing(const ComputedStyle& style) const {
    if (style.fontSize <= 0.f) return 0.f;
    const LLFontGL& font = fontForStyle(style);
    return usedTextSpacing(style, font).letter;
}

void OpenGLPaintContext::Impl::beginFrame(const PaintTarget& target) {
    mFrameNativeAppearance = target.nativeAppearance;
    effects.resetFrame();
    clipStack.beginFrame(target);
    geometry.beginFrame(target);
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
    mFrameNativeAppearance = nullptr;
}

Vec2 OpenGLPaintContext::Impl::measureText(const std::string& textValue, const ComputedStyle& style) const {
    return text.measureText(textValue, style);
}

float OpenGLPaintContext::Impl::usedLetterSpacing(const ComputedStyle& style) const {
    return text.usedLetterSpacing(style);
}

void OpenGLPaintContext::Impl::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    clipStack.push(rect, scale, axes);
}

void OpenGLPaintContext::Impl::popClip() {
    clipStack.pop();
}

void OpenGLPaintContext::Impl::pushTranslation(const Vec2& translation) {
    clipStack.pushTranslation(translation);
}

void OpenGLPaintContext::Impl::popTranslation() {
    clipStack.popTranslation();
}

void OpenGLPaintContext::Impl::beginEffects(const Rect& rect, const ComputedStyle& style, float scale) {
    effects.begin(rect, style, scale);
}

void OpenGLPaintContext::Impl::endEffects() {
    effects.end();
}

void OpenGLPaintContext::Impl::paintNativeScrollbar(const NativeScrollbarPaintRequest& request) {
    const NativeAppearance& appearance = nativeAppearance();
    const std::optional<ResolvedScrollbarClip> scrollbarClip = resolveScrollbarClip(request.clip);
    const ResolvedScrollbarClip* clip = scrollbarClip ? &*scrollbarClip : nullptr;
    const auto paint = [this, clip](const Rect& rect, Color color, float radius) {
        if (rect.empty() || color.a <= 0.f) return;
        const Rect box = snapped(rect);
        geometry.drawRoundedShape(PaintOp::Fill, box, resolveBorderRadii(box, BorderRadii::uniform(Length{radius})), 0.f, color, OutlineStyle::Solid,
                                  std::nullopt, nullptr, clip);
    };
    const auto paintArrow = [this, clip](const Rect& rect, ScrollbarAxis axis, bool pointsPositive, Color color) {
        geometry.drawArrow(rect, axis, pointsPositive, color, clip);
    };
    const auto paintAxis = [&](const ScrollbarAxisGeometry& axis) {
        if (!axis.visible) return;
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

void OpenGLPaintContext::Impl::paintBox(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap) {
    geometry.paintBox(rect, style, topBorderGap);
}

void OpenGLPaintContext::Impl::paintText(const std::string& textValue, const Rect& rect, const ComputedStyle& style) {
    text.paintText(textValue, rect, style);
}

void OpenGLPaintContext::Impl::paintNativeInputMark(const NativeInputMarkPaintRequest& request) {
    geometry.drawNativeInputMark(request);
}

void OpenGLPaintContext::Impl::paintIcon(const std::string& name, const Rect& rect, const ComputedStyle& style, float scale) {
    icons.paintIcon(system.icon(name), rect, style, scale);
}

bool EffectRenderer::captureFramebuffer(const Rect& capture, float scale, LLRenderTarget& target) {
    const U32 width = static_cast<U32>(std::max(1.f, std::round(capture.w * scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(capture.h * scale)));
    if (!ensureTarget(target, width, height)) return false;

    const paint::PaintState state = mClips.snapshot();
    const S32 sourceX = ll_round(state.target.pixelOrigin.x + (capture.x - state.origin.x) * scale);
    const S32 sourceY = ll_round(state.target.pixelOrigin.y + (capture.y - state.origin.y) * scale);
    gGL.flush();
    target.bindTexture(0, 0, ALSamplers::BilinearClamp);
    glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, sourceX, sourceY, static_cast<GLsizei>(width), static_cast<GLsizei>(height));
    return true;
}

LLRenderTarget* EffectRenderer::applyBlur(LLRenderTarget& source, LLRenderTarget& horizontalTarget, LLRenderTarget& verticalTarget,
                                          const Rect& capture, const Rect& effectRect, const Effect& effect, float scale) {
    if (!mProgram.mProgramObject) return &source;
    const U32 width = source.getWidth();
    const U32 height = source.getHeight();
    if (!ensureTarget(horizontalTarget, width, height) || !ensureTarget(verticalTarget, width, height)) return &source;

    constexpr float kRadiansPerDegree = std::numbers::pi_v<float> / 180.f;
    const float angle = effect.angleDegrees * kRadiansPerDegree;
    const Vec2 direction(std::sin(angle), std::cos(angle));
    const float extent = (std::abs(direction.x) * effectRect.w + std::abs(direction.y) * effectRect.h) * scale;
    const Vec2 center((effectRect.x + effectRect.w * .5f - capture.x) * scale, (effectRect.y + effectRect.h * .5f - capture.y) * scale);
    const Vec2 gradientLineStart = center - direction * (extent * .5f);
    const Vec2 gradientLine = direction * extent;
    const Vec2 gradientStart = gradientLineStart + gradientLine * effect.startPosition;
    Vec2 gradientEnd = gradientLineStart + gradientLine * effect.endPosition;
    if (effect.startPosition == effect.endPosition) gradientEnd = gradientStart + direction * std::max(1.f, scale);
    const float maximumRadius = static_cast<float>(std::max(width, height));

    const PaintShaderUniforms& uniforms = shaderUniforms();

    auto pass = [&](LLRenderTarget& input, LLRenderTarget& output, float axisX, float axisY) {
        LLGLDisable disableScissor(GL_SCISSOR_TEST);
        LLGLDisable disableBlend(GL_BLEND);
        gGL.flush();
        paint::RenderTargetGuard targetGuard(output);
        paint::MatrixGuard matrixGuard({0.f, 0.f, capture.w, capture.h}, scale);
        mProgram.bind();
        setPaintOp(mProgram, PaintOp::Blur);
        setClipCoverageUniforms(mProgram, std::nullopt);
        mProgram.uniform2f(uniforms.effectTextureSize, static_cast<float>(width), static_cast<float>(height));
        mProgram.uniform2f(uniforms.effectBlurAxis, axisX, axisY);
        mProgram.uniform2f(uniforms.effectBlurRadii, std::min(effect.startRadius * scale, maximumRadius),
                           std::min(effect.endRadius * scale, maximumRadius));
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

void EffectRenderer::compositeEffect(LLRenderTarget& source, const Rect& capture, const Rect& destination, const ResolvedBorderRadii& radii,
                                     bool roundedMask) {
    const Rect visible = intersectRects(capture, destination);
    if (!mProgram.mProgramObject || visible.empty() || capture.empty()) return;
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

void EffectRenderer::begin(const Rect& rect, const ComputedStyle& style, float scale) {
    if (mEffectDepth == mEffectLayers.size()) mEffectLayers.emplace_back();
    EffectLayer& frame = mEffectLayers[mEffectDepth++];
    frame.layerEffects.clear();
    frame.effectRect = rect;
    frame.scale = std::max(scale, .0001f);
    frame.capture.reset();
    const float maximumPadding = std::max(mClips.bounds().w, mClips.bounds().h);

    auto captureBounds = [&](float padding) {
        const Rect expanded = snappedOutward(expandedRect(rect, padding), frame.scale);
        return intersectRects(expanded, mClips.bounds());
    };

    for (const Effect& effect : style.effects) {
        if (effect.startRadius <= 0.f && effect.endRadius <= 0.f) continue;
        if (effect.kind == EffectKind::LayerBlur) {
            frame.layerEffects.push_back(effect);
            continue;
        }
        const float padding = std::min(std::max(effect.startRadius, effect.endRadius) * 2.f + 1.f / frame.scale, maximumPadding);
        const Rect capture = captureBounds(padding);
        if (capture.empty()) continue;
        if (!captureFramebuffer(capture, frame.scale, mBackgroundTargets[0])) continue;
        LLRenderTarget* blurred = applyBlur(mBackgroundTargets[0], mBackgroundTargets[1], mBackgroundTargets[2], capture, rect, effect, frame.scale);
        compositeEffect(*blurred, capture, rect, resolveBorderRadii(rect, style.borderRadius), true);
    }

    if (frame.layerEffects.empty()) return;
    float padding = 1.f / frame.scale;
    for (const Effect& effect : frame.layerEffects)
        padding = std::min(padding + std::max(effect.startRadius, effect.endRadius) * 2.f, maximumPadding);
    frame.captureRect = captureBounds(padding);
    if (frame.captureRect.empty()) return;

    const U32 width = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.w * frame.scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.h * frame.scale)));
    if (!ensureTarget(frame.targets[0], width, height)) return;
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
    if (!frame.capture) return;

    gGL.flush();
    frame.capture.reset();
    mClips.reapply();

    LLRenderTarget* source = &frame.targets[0];
    for (const Effect& effect : frame.layerEffects) {
        LLRenderTarget& horizontal = frame.targets[1];
        LLRenderTarget& vertical = source == &frame.targets[0] ? frame.targets[2] : frame.targets[0];
        source = applyBlur(*source, horizontal, vertical, frame.captureRect, frame.effectRect, effect, frame.scale);
    }
    compositeEffect(*source, frame.captureRect, frame.captureRect, uniformBorderRadii(0.f), false);
}

void GeometryPainter::prepareVectorDraw() {
    gGL.getTextureSlot(0)->unbind();
    if (program.mProgramObject) {
        program.bind();
        setPaintOp(program, PaintOp::Direct);
        applyClipCoverage(program);
    }
}

void TextPainter::prepareTextDraw() {
    gUIProgram.bind();
}

void GeometryPainter::drawMesh(const Mesh& mesh) {
    if (mesh.empty() || !program.mProgramObject) return;
    prepareVectorDraw();
    gGL.begin(LLRender::TRIANGLES);
    for (const Vertex& vertex : mesh.vertices) {
        gGL.color4f(vertex.color.r, vertex.color.g, vertex.color.b, vertex.color.a);
        gGL.vertex2f(vertex.position.x, vertex.position.y);
    }
    gGL.end();
}

void GeometryPainter::drawArrow(const Rect& rect, ScrollbarAxis axis, bool pointsPositive, const Color& color, const ResolvedScrollbarClip* clip) {
    const Rect box = snapped(rect);
    if (!program.mProgramObject || box.empty() || color.a <= 0.f) return;
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
    const Rect bounds = snapped(request.bounds);
    if (!program.mProgramObject || bounds.empty() || request.color.a <= 0.f) return;

    if (request.mark == NativeInputMark::Dash) {
        const float radius = std::min(request.radius, std::min(bounds.w, bounds.h) * .5f);
        drawRoundedShape(PaintOp::Fill, bounds, uniformBorderRadii(radius), 0.f, request.color);
        return;
    }

    if (request.strokeWidth <= 0.f) return;
    if (request.path.empty()) return;
    drawMesh(tessellateStroke(request.path, request.color, request.strokeWidth, coverageFringeWidth(request.scale), StrokeCap::Butt));
}

void TextPainter::paintText(const std::string& text, const Rect& rect, const ComputedStyle& style) {
    if (text.empty() || style.fontSize <= 0.f || style.color.a <= 0.f) return;
    const LLFontGL& font = fontForStyle(style);
    prepareTextDraw();
    geometry.applyClipCoverage(gUIProgram);
    const LLVector3 uiTranslation = gGL.getUITranslation();
    const Rect glyphRect{rect.x + uiTranslation.mV[VX], rect.y + uiTranslation.mV[VY], rect.w, rect.h};
    const LLFontGL::HAlign horizontal = horizontalAlignment(style);
    constexpr LLFontGL::VAlign vertical = LLFontGL::VCENTER;
    const LLColor4 color(style.color.r, style.color.g, style.color.b, style.color.a);
    const LLFontGL::TextSpacing spacing = usedTextSpacing(style, font);
    const U8 fontStyle = style.textDecoration == TextDecoration::Underline ? LLFontGL::UNDERLINE : LLFontGL::NORMAL;
    font.renderUTF8(text, 0, textX(glyphRect, horizontal), textY(glyphRect, vertical), color, horizontal, vertical, fontStyle, LLFontGL::NO_SHADOW,
                    S32_MAX, S32_MAX, nullptr, false, true, spacing);
    if (style.textDecoration == TextDecoration::LineThrough) {
        const float width = measureOpenGLText(text, style).x;
        const float anchor = textX(rect, horizontal);
        const float left = horizontal == LLFontGL::RIGHT ? anchor - width : horizontal == LLFontGL::HCENTER ? anchor - width * .5f : anchor;
        const float thickness = std::max(1.f, std::round(font.getLineHeight() / 14.f));
        const float y = textBaseline(rect, vertical, font) + font.getAscenderHeight() * .3f;
        geometry.drawRoundedShape(PaintOp::Fill, {left, y - thickness * .5f, width, thickness}, uniformBorderRadii(0.f), 0.f, style.color);
    }
}

void GeometryPainter::drawRoundedShape(PaintOp op, const Rect& rect, const ResolvedBorderRadii& radii, float borderWidth, const Color& color,
                                       OutlineStyle outlineStyle, std::optional<TopBorderGap> topBorderGap, const ResolvedBorderRadii* innerRadii,
                                       const ResolvedScrollbarClip* clip) {
    if (!program.mProgramObject || rect.empty() || color.a <= 0.f || (op == PaintOp::Border && borderWidth <= 0.f)) return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const ResolvedBorderRadii& inner = innerRadii ? *innerRadii : radii;
    const float padding = coverageFringe();
    const Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
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

void GeometryPainter::drawRoundedGradient(const Rect& rect, const ResolvedBorderRadii& radii, const Gradient& gradient,
                                          const EdgeInsets* borderWidths, std::optional<TopBorderGap> topBorderGap,
                                          const ResolvedBorderRadii* innerRadii) {
    if (!program.mProgramObject || rect.empty() || gradient.stops.size() < 2 || gradient.stops.size() > 8 || (borderWidths && !borderWidths->any()))
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const ResolvedBorderRadii& inner = innerRadii ? *innerRadii : radii;

    constexpr float kRadiansPerDegree = std::numbers::pi_v<float> / 180.f;
    const float angle = gradient.angleDegrees * kRadiansPerDegree;
    const Vec2 direction(std::sin(angle), std::cos(angle));
    const float extent = std::abs(direction.x) * rect.w + std::abs(direction.y) * rect.h;
    const Vec2 center(rect.w * .5f, rect.h * .5f);
    const Vec2 start = center - direction * (extent * .5f);
    const Vec2 end = center + direction * (extent * .5f);
    const Vec2 gradientCenterValue(gradient.center.x * rect.w, gradient.center.y * rect.h);
    const float farX = std::max(gradientCenterValue.x, rect.w - gradientCenterValue.x);
    const float farY = std::max(gradientCenterValue.y, rect.h - gradientCenterValue.y);
    Vec2 radialRadius;
    if (gradient.radialShape == RadialGradientShape::Circle) {
        const float farCorner = std::sqrt(farX * farX + farY * farY);
        radialRadius = {farCorner, farCorner};
    } else {
        constexpr float kSquareRootTwo = 1.41421356237f;
        radialRadius = {farX * kSquareRootTwo, farY * kSquareRootTwo};
    }
    constexpr std::size_t kMaxGradientStops = 8;
    std::array<GLfloat, kMaxGradientStops * 4> colors{};
    std::array<GLfloat, kMaxGradientStops> stops{};
    for (std::size_t index = 0; index < gradient.stops.size(); ++index) {
        const GradientStop& stop = gradient.stops[index];
        colors[index * 4] = stop.color.r;
        colors[index * 4 + 1] = stop.color.g;
        colors[index * 4 + 2] = stop.color.b;
        colors[index * 4 + 3] = stop.color.a;
        stops[index] = stop.position;
    }

    const float padding = coverageFringe();
    const Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, borderWidths ? PaintOp::GradientBorder : PaintOp::Gradient);
    program.uniform4f(uniforms.shapeRect, rect.x, rect.y, rect.w, rect.h);
    setBorderRadiusUniforms(program, uniforms, radii, false);
    setBorderRadiusUniforms(program, uniforms, inner, true);
    program.uniform2f(uniforms.shapeOffset, padding, padding);
    if (borderWidths) program.uniform4f(uniforms.borderWidths, borderWidths->top, borderWidths->right, borderWidths->bottom, borderWidths->left);
    program.uniform2f(uniforms.topBorderGap, topBorderGap ? topBorderGap->left - rect.left() : -1.f,
                      topBorderGap ? topBorderGap->right - rect.left() : -1.f);
    program.uniform1i(uniforms.gradientKind, gradientOpValue(gradient.kind));
    program.uniform1i(uniforms.gradientRepeating, gradient.repeating ? 1 : 0);
    program.uniform2f(uniforms.gradientStart, start.x, start.y);
    program.uniform2f(uniforms.gradientEnd, end.x, end.y);
    program.uniform2f(uniforms.gradientCenter, gradientCenterValue.x, gradientCenterValue.y);
    program.uniform2f(uniforms.gradientRadius, radialRadius.x, radialRadius.y);
    program.uniform1f(uniforms.gradientAngle, gradient.angleDegrees);
    program.uniform1i(uniforms.gradientStopCount, static_cast<GLint>(gradient.stops.size()));
    program.uniform4fv(uniforms.gradientColors, static_cast<U32>(gradient.stops.size()), colors.data());
    program.uniform1fv(uniforms.gradientStops, static_cast<U32>(gradient.stops.size()), stops.data());
    drawShapeQuad(quad);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawShadow(const Rect& rect, const ResolvedBorderRadii& radii, const BoxShadow& shadow) {
    if (!program.mProgramObject || rect.empty() || shadow.color.a <= 0.f) return;
    const PaintShaderUniforms& uniforms = shaderUniforms();

    const Rect box = rect;
    Rect shape = box;
    Rect quad = box;
    Vec2 localShapeOffset;
    ResolvedBorderRadii shapeRadii = radii;
    ResolvedBorderRadii innerRadii = radii;
    PaintOp op = PaintOp::InsetShadow;
    if (!shadow.inset) {
        op = PaintOp::OuterShadow;
        shape = {box.x + shadow.horizontal - shadow.spread, box.y - shadow.vertical - shadow.spread, std::max(0.f, box.w + shadow.spread * 2.f),
                 std::max(0.f, box.h + shadow.spread * 2.f)};
        if (shape.empty()) return;
        shapeRadii = expandedBorderRadii(radii, shadow.spread, shape.w, shape.h);
        innerRadii = shapeRadii;
        const float padding = shadow.blur * 2.f + coverageFringe();
        quad = {shape.x - padding, shape.y - padding, shape.w + padding * 2.f, shape.h + padding * 2.f};
        localShapeOffset = {padding, padding};
    } else {
        const Rect hole = {0.f, 0.f, std::max(0.f, box.w - shadow.spread * 2.f), std::max(0.f, box.h - shadow.spread * 2.f)};
        innerRadii = insetBorderRadii(radii, {shadow.spread, shadow.spread, shadow.spread, shadow.spread}, hole.w, hole.h);
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
    program.uniform4f(uniforms.shapeColor, shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a);
    program.uniform2f(uniforms.shapeOffset, localShapeOffset.x, localShapeOffset.y);
    program.uniform2f(uniforms.shadowOffset, shadow.horizontal, -shadow.vertical);
    program.uniform1f(uniforms.shadowBlur, shadow.blur);
    program.uniform1f(uniforms.shadowSpread, shadow.spread);
    drawShapeQuad(quad);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawShapeQuad(const Rect& rect) {
    gGL.begin(LLRender::TRIANGLES);
    gGL.color4f(1.f, 1.f, 1.f, 1.f);
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

void GeometryPainter::drawBorder(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap) {
    if (!style.borderWidth.any()) return;
    const Rect box = rect;
    const ResolvedBorderRadii borderRadii = resolveBorderRadii(box, style.borderRadius);
    const bool square = borderRadii.topLeft.x == 0.f
        && borderRadii.topLeft.y == 0.f
        && borderRadii.topRight.x == 0.f
        && borderRadii.topRight.y == 0.f
        && borderRadii.bottomRight.x == 0.f
        && borderRadii.bottomRight.y == 0.f
        && borderRadii.bottomLeft.x == 0.f
        && borderRadii.bottomLeft.y == 0.f;
    if (style.borderStyle != BorderStyle::Solid
        && style.borderWidth.isUniform()
        && !style.borderGradient
        && square
        && (!topBorderGap || topBorderGap->empty())) {
        const float width = style.borderWidth.top;
        const Color highlight = shade(style.borderColor, Color(1.f, 1.f, 1.f, style.borderColor.a), .45f);
        const Color shadow = shade(style.borderColor, Color(0.f, 0.f, 0.f, style.borderColor.a), .45f);
        const bool outset = style.borderStyle == BorderStyle::Outset;
        const Color topLeft = outset ? highlight : shadow;
        const Color bottomRight = outset ? shadow : highlight;
        const ResolvedBorderRadii zeroRadii = uniformBorderRadii(0.f);
        drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width, box.w, width}, zeroRadii, 0.f, topLeft);
        drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom(), box.w, width}, zeroRadii, 0.f, bottomRight);
        const float height = std::max(0.f, box.h - width * 2.f);
        drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom() + width, width, height}, zeroRadii, 0.f, topLeft);
        drawRoundedShape(PaintOp::Fill, {box.right() - width, box.bottom() + width, width, height}, zeroRadii, 0.f, bottomRight);
        return;
    }
    const Rect innerBox = insetRect(box, style.borderWidth);
    const ResolvedBorderRadii innerRadii = insetBorderRadii(borderRadii, style.borderWidth, innerBox.w, innerBox.h);
    if (style.borderGradient) {
        drawRoundedGradient(box, borderRadii, *style.borderGradient, &style.borderWidth, topBorderGap, &innerRadii);
        return;
    }
    if (style.borderColor.a <= 0.f) return;
    if (style.borderWidth.isUniform()) {
        drawRoundedShape(PaintOp::Border, box, borderRadii, style.borderWidth.top, style.borderColor, OutlineStyle::Solid, topBorderGap, &innerRadii);
        return;
    }
    const EdgeInsets& width = style.borderWidth;
    const ResolvedBorderRadii zeroRadii = uniformBorderRadii(0.f);
    if (topBorderGap && !topBorderGap->empty()) {
        const float gapLeft = std::clamp(topBorderGap->left, box.left(), box.right());
        const float gapRight = std::clamp(topBorderGap->right, gapLeft, box.right());
        drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width.top, std::max(0.f, gapLeft - box.left()), width.top}, zeroRadii, 0.f,
                         style.borderColor);
        drawRoundedShape(PaintOp::Fill, {gapRight, box.top() - width.top, std::max(0.f, box.right() - gapRight), width.top}, zeroRadii, 0.f,
                         style.borderColor);
    } else drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width.top, box.w, width.top}, zeroRadii, 0.f, style.borderColor);
    drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom(), box.w, width.bottom}, zeroRadii, 0.f, style.borderColor);
    drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom() + width.bottom, width.left, box.h - width.top - width.bottom}, zeroRadii, 0.f,
                     style.borderColor);
    drawRoundedShape(PaintOp::Fill, {box.right() - width.right, box.bottom() + width.bottom, width.right, box.h - width.top - width.bottom},
                     zeroRadii, 0.f, style.borderColor);
}

void GeometryPainter::drawOutline(const Rect& rect, const ComputedStyle& style) {
    if (style.outline.width <= 0.f || style.outline.color.a <= 0.f) return;
    const float width = style.outline.width;
    const float expansion = width + style.outline.offset;
    const Rect box = rect;
    const Rect outlineBox = {box.x - expansion, box.y - expansion, box.w + expansion * 2.f, box.h + expansion * 2.f};
    const ResolvedBorderRadii outlineRadii = expandedBorderRadii(resolveBorderRadii(box, style.borderRadius), expansion, outlineBox.w, outlineBox.h);
    const Rect innerBox = insetRect(outlineBox, {width, width, width, width});
    const ResolvedBorderRadii innerRadii = insetBorderRadii(outlineRadii, {width, width, width, width}, innerBox.w, innerBox.h);
    drawRoundedShape(PaintOp::Border, outlineBox, outlineRadii, width, style.outline.color, style.outline.style, std::nullopt, &innerRadii);
}

void GeometryPainter::paintBox(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap) {
    const Rect box = rect;
    const ResolvedBorderRadii borderRadii = resolveBorderRadii(box, style.borderRadius);
    for (auto shadow = style.shadows.rbegin(); shadow != style.shadows.rend(); ++shadow)
        if (!shadow->inset) drawShadow(rect, borderRadii, *shadow);

    Rect fillBox = box;
    ResolvedBorderRadii fillRadii = borderRadii;
    const bool bordered = hasVisibleBorder(style);
    if (bordered) {
        if (style.borderStyle == BorderStyle::Solid && hasOpaqueBackground(style) && (!topBorderGap || topBorderGap->empty()))
            if (style.borderGradient) drawRoundedGradient(fillBox, borderRadii, *style.borderGradient);
            else drawRoundedShape(PaintOp::Fill, fillBox, borderRadii, 0.f, style.borderColor);
        else drawBorder(rect, style, topBorderGap);
        fillBox = insetRect(fillBox, style.borderWidth);
        fillRadii = insetBorderRadii(borderRadii, style.borderWidth, fillBox.w, fillBox.h);
    }
    if (style.backgroundColor.a > 0.f) drawRoundedShape(PaintOp::Fill, fillBox, fillRadii, 0.f, style.backgroundColor);
    if (style.backgroundGradient) drawRoundedGradient(fillBox, fillRadii, *style.backgroundGradient);
    for (auto shadow = style.shadows.rbegin(); shadow != style.shadows.rend(); ++shadow)
        if (shadow->inset) drawShadow(fillBox, fillRadii, *shadow);
    if (!bordered) drawBorder(rect, style, topBorderGap);
    drawOutline(rect, style);
}

void IconPainter::paintIcon(const SvgIcon* icon, const Rect& rect, const ComputedStyle& style, float scale) {
    if (!icon || icon->empty()) return;
    const Rect source = icon->viewBox;
    const Color color = style.iconStrokeColor.a > 0.f ? style.iconStrokeColor : style.backgroundColor;
    const float unitScale = std::min(rect.w / std::max(0.0001f, source.w), rect.h / std::max(0.0001f, source.h));
    const float width = style.svgStrokeWidth ? style.svgStrokeWidth->pixels : icon->strokeWidth * unitScale;
    const StrokeCap cap = style.svgStrokeCapSet ? style.svgStrokeCap : icon->strokeCap;

    auto drawPath = [&](const Path& sourcePath) {
        geometry.drawMesh(tessellateStroke(transformSvgPath(sourcePath, source, rect), color, width, coverageFringeWidth(scale), cap));
    };
    for (const Path& iconPath : icon->paths) drawPath(iconPath);
}
} // namespace radia::ui
