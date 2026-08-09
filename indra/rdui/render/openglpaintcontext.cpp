/**
 * @file openglpaintcontext.cpp
 * @brief Implements retained UI painting on the OpenGL renderer.
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

namespace rdui {
namespace {
Rect snapped(const Rect& rect) {
    const float left = std::round(rect.left());
    const float right = std::round(rect.right());
    const float bottom = std::round(rect.bottom());
    const float top = std::round(rect.top());
    return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

bool hasVisibleBorder(const Style& style) {
    return style.border_width.any() && (style.border_gradient.has_value() || style.border_color.a > 0.f);
}

bool hasOpaqueBackground(const Style& style) {
    if (!style.background_gradient) return style.background_color.a >= 1.f;
    return std::all_of(style.background_gradient->stops.begin(), style.background_gradient->stops.end(),
                       [](const GradientStop& stop) { return stop.color.a >= 1.f; });
}

Rect expandedRect(const Rect& rect, float amount) {
    return {rect.x - amount, rect.y - amount, rect.w + amount * 2.f, rect.h + amount * 2.f};
}

Rect snappedOutward(const Rect& rect, float scale) {
    const float safe_scale = std::max(scale, .0001f);
    const float left = std::floor(rect.left() * safe_scale) / safe_scale;
    const float right = std::ceil(rect.right() * safe_scale) / safe_scale;
    const float bottom = std::floor(rect.bottom() * safe_scale) / safe_scale;
    const float top = std::ceil(rect.top() * safe_scale) / safe_scale;
    return {left, bottom, std::max(0.f, right - left), std::max(0.f, top - bottom)};
}

bool ensureTarget(LLRenderTarget& target, U32 width, U32 height) {
    width = std::max<U32>(1, width);
    height = std::max<U32>(1, height);
    if (!target.isComplete()) return target.allocate(width, height, GL_RGBA8);
    if (target.getWidth() != width || target.getHeight() != height) target.resize(width, height);
    return target.isComplete() && target.getWidth() == width && target.getHeight() == height;
}

LLFontGL::HAlign horizontalAlignment(const Style& style) {
    return style.text_align == TextAlign::Center ? LLFontGL::HCENTER : style.text_align == TextAlign::Right ? LLFontGL::RIGHT : LLFontGL::LEFT;
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

const LLFontGL& fontForStyle(const Style& style) {
    LLFontGL* font = LLFontGL::getFontAtPixelSize("SansSerif", style.font_size, style.font_weight, style.font_italic);
    if (!font) LL_ERRS("rdui") << "OpenGL text adapter used before viewer fonts were initialized." << LL_ENDL;
    return *font;
}

float textLineHeight(const Style& style) {
    if (style.line_height) return std::ceil(style.line_height->pixels);
    if (style.font_size <= 0.f) return 0.f;
    return static_cast<float>(fontForStyle(style).getLineHeight());
}

LLFontGL::TextSpacing usedTextSpacing(const Style& style, const LLFontGL& font) {
    static const LLWString space(U" ");
    const float space_advance = std::max(0.f, font.getWidthF32(space.c_str(), 0, 1, true));
    return {
        style.letter_spacing.resolve(space_advance),
        style.word_spacing.resolve(style.font_size),
    };
}

Vec2 measureOpenGLText(const std::string& text, const Style& style) {
    if (style.font_size <= 0.f) return {0.f, textLineHeight(style)};
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
    LLStaticHashedString shapeRadius{"shapeRadius"};
    LLStaticHashedString shapeBorderWidth{"shapeBorderWidth"};
    LLStaticHashedString shapeColor{"shapeColor"};
    LLStaticHashedString shapeOffset{"shapeOffset"};
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
    LLStaticHashedString effectMaskRadius{"effectMaskRadius"};
    LLStaticHashedString effectRoundedMask{"effectRoundedMask"};
};

const PaintShaderUniforms& shaderUniforms() {
    static const PaintShaderUniforms uniforms;
    return uniforms;
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
} // namespace

struct GeometryPainter {
    explicit GeometryPainter(::LLGLSLShader& shape_program) : program(shape_program) {}

    void drawMesh(const Mesh& mesh);
    void drawRoundedShape(PaintOp op, const Rect& rect, float radius, float border_width, const Color& color,
                          OutlineStyle outline_style = OutlineStyle::Solid, std::optional<TopBorderGap> top_border_gap = std::nullopt);
    void drawRoundedGradient(const Rect& rect, float radius, const Gradient& gradient, const EdgeInsets* border_widths = nullptr,
                             std::optional<TopBorderGap> top_border_gap = std::nullopt);
    void drawShadow(const Rect& rect, float radius, const BoxShadow& shadow);
    void drawBorder(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap = std::nullopt);
    void drawOutline(const Rect& rect, const Style& style);
    void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap);
    void prepareVectorDraw();
    static void drawShapeQuad(const Rect& rect);

    ::LLGLSLShader& program;
};

struct TextPainter {
    explicit TextPainter(GeometryPainter& geometry_painter) : geometry(geometry_painter) {}

    Vec2 measureText(const std::string& text, const Style& style) const;
    float usedLetterSpacing(const Style& style) const;
    void paintText(const std::string& text, const Rect& rect, const Style& style);
    static void prepareTextDraw();

    GeometryPainter& geometry;
};

struct IconPainter {
    explicit IconPainter(GeometryPainter& geometry_painter) : geometry(geometry_painter) {}

    void paintIcon(const SvgIcon* icon, const Rect& rect, const Style& style, float scale);

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
    EffectRenderer(::LLGLSLShader& shape_program, paint::ClipStack& clip_stack) : mProgram(shape_program), mClips(clip_stack) {}

    void begin(const Rect& rect, const Style& style, float scale);
    void end();
    void resetFrame() {
        while (mEffectDepth > 0) mEffectLayers[--mEffectDepth].capture.reset();
        for (EffectLayer& layer : mEffectLayers) layer.capture.reset();
        mEffectDepth = 0;
    }
    std::size_t depth() const { return mEffectDepth; }

private:
    bool captureFramebuffer(const Rect& capture, float scale, LLRenderTarget& target);
    LLRenderTarget* applyBlur(LLRenderTarget& source, LLRenderTarget& horizontal_target, LLRenderTarget& vertical_target, const Rect& capture,
                              const Rect& effect_rect, const Effect& effect, float scale);
    void compositeEffect(LLRenderTarget& source, const Rect& capture, const Rect& destination, float radius, bool rounded_mask);

    ::LLGLSLShader& mProgram;
    paint::ClipStack& mClips;
    std::array<LLRenderTarget, 3> mBackgroundTargets;
    std::deque<EffectLayer> mEffectLayers;
    std::size_t mEffectDepth = 0;
};

struct OpenGLPaintContext::Impl {
    Impl(::LLGLSLShader& shape_program, const System& system)
        : system(system), geometry(shape_program), text(geometry), icons(geometry), effects(shape_program, clipStack) {}

    void beginFrame();
    void endFrame();
    Vec2 measureText(const std::string& text, const Style& style) const;
    float usedLetterSpacing(const Style& style) const;
    void pushClip(const Rect& rect, float scale, ClipAxes axes);
    void popClip();
    void beginEffects(const Rect& rect, const Style& style, float scale);
    void endEffects();
    void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap);
    void paintText(const std::string& text, const Rect& rect, const Style& style);
    void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale);

    std::unique_ptr<LLGLSUIDefault> uiState;
    const System& system;
    paint::ClipStack clipStack;
    GeometryPainter geometry;
    TextPainter text;
    IconPainter icons;
    EffectRenderer effects;
};

OpenGLPaintContext::OpenGLPaintContext(::LLGLSLShader& shape_program, const System& system) : mImpl(std::make_unique<Impl>(shape_program, system)) {}

OpenGLPaintContext::~OpenGLPaintContext() = default;

void OpenGLPaintContext::beginFrame() {
    mImpl->beginFrame();
}

void OpenGLPaintContext::endFrame() {
    mImpl->endFrame();
}

Vec2 OpenGLPaintContext::measureText(const std::string& text, const Style& style) const {
    return mImpl->measureText(text, style);
}

float OpenGLPaintContext::usedLetterSpacing(const Style& style) const {
    return mImpl->usedLetterSpacing(style);
}

std::uint64_t OpenGLPaintContext::generation() const {
    return mImpl->system.generation();
}

void OpenGLPaintContext::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    mImpl->pushClip(rect, scale, axes);
}

void OpenGLPaintContext::popClip() {
    mImpl->popClip();
}

void OpenGLPaintContext::beginEffects(const Rect& rect, const Style& style, float scale) {
    mImpl->beginEffects(rect, style, scale);
}

void OpenGLPaintContext::endEffects() {
    mImpl->endEffects();
}

void OpenGLPaintContext::paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    mImpl->paintBox(rect, style, top_border_gap);
}

void OpenGLPaintContext::paintText(const std::string& text, const Rect& rect, const Style& style) {
    mImpl->paintText(text, rect, style);
}

void OpenGLPaintContext::paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) {
    mImpl->paintIcon(name, rect, style, scale);
}

Vec2 TextPainter::measureText(const std::string& text, const Style& style) const {
    return measureOpenGLText(text, style);
}

float TextPainter::usedLetterSpacing(const Style& style) const {
    if (style.font_size <= 0.f) return 0.f;
    const LLFontGL& font = fontForStyle(style);
    return usedTextSpacing(style, font).letter;
}

void OpenGLPaintContext::Impl::beginFrame() {
    effects.resetFrame();
    clipStack.beginFrame();
    uiState = std::make_unique<LLGLSUIDefault>();
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
}

void OpenGLPaintContext::Impl::endFrame() {
    if (effects.depth() != 0) {
        llassert(false);
        effects.resetFrame();
    }
    clipStack.popAll();
    gUIProgram.bind();
    uiState.reset();
}

Vec2 OpenGLPaintContext::Impl::measureText(const std::string& text_value, const Style& style) const {
    return text.measureText(text_value, style);
}

float OpenGLPaintContext::Impl::usedLetterSpacing(const Style& style) const {
    return text.usedLetterSpacing(style);
}

void OpenGLPaintContext::Impl::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    clipStack.push(rect, scale, axes);
}

void OpenGLPaintContext::Impl::popClip() {
    clipStack.pop();
}

void OpenGLPaintContext::Impl::beginEffects(const Rect& rect, const Style& style, float scale) {
    effects.begin(rect, style, scale);
}

void OpenGLPaintContext::Impl::endEffects() {
    effects.end();
}

void OpenGLPaintContext::Impl::paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    geometry.paintBox(rect, style, top_border_gap);
}

void OpenGLPaintContext::Impl::paintText(const std::string& text_value, const Rect& rect, const Style& style) {
    text.paintText(text_value, rect, style);
}

void OpenGLPaintContext::Impl::paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) {
    icons.paintIcon(system.icon(name), rect, style, scale);
}

bool EffectRenderer::captureFramebuffer(const Rect& capture, float scale, LLRenderTarget& target) {
    const U32 width = static_cast<U32>(std::max(1.f, std::round(capture.w * scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(capture.h * scale)));
    if (!ensureTarget(target, width, height)) return false;

    const paint::PaintState state = mClips.snapshot();
    const S32 source_x = ll_round(state.pixelOrigin.x + (capture.x - state.origin.x) * scale);
    const S32 source_y = ll_round(state.pixelOrigin.y + (capture.y - state.origin.y) * scale);
    gGL.flush();
    target.bindTexture(0, 0, LLTexUnit::TFO_BILINEAR);
    gGL.getTexUnit(0)->setTextureAddressMode(LLTexUnit::TAM_CLAMP);
    glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, source_x, source_y, static_cast<GLsizei>(width), static_cast<GLsizei>(height));
    return true;
}

LLRenderTarget* EffectRenderer::applyBlur(LLRenderTarget& source, LLRenderTarget& horizontal_target, LLRenderTarget& vertical_target,
                                          const Rect& capture, const Rect& effect_rect, const Effect& effect, float scale) {
    if (!mProgram.mProgramObject) return &source;
    const U32 width = source.getWidth();
    const U32 height = source.getHeight();
    if (!ensureTarget(horizontal_target, width, height) || !ensureTarget(vertical_target, width, height)) return &source;

    constexpr float radians_per_degree = std::numbers::pi_v<float> / 180.f;
    const float angle = effect.angle_degrees * radians_per_degree;
    const Vec2 direction(std::sin(angle), std::cos(angle));
    const float extent = (std::abs(direction.x) * effect_rect.w + std::abs(direction.y) * effect_rect.h) * scale;
    const Vec2 center((effect_rect.x + effect_rect.w * .5f - capture.x) * scale, (effect_rect.y + effect_rect.h * .5f - capture.y) * scale);
    const Vec2 gradient_line_start = center - direction * (extent * .5f);
    const Vec2 gradient_line = direction * extent;
    const Vec2 gradient_start = gradient_line_start + gradient_line * effect.start_position;
    Vec2 gradient_end = gradient_line_start + gradient_line * effect.end_position;
    if (effect.start_position == effect.end_position) gradient_end = gradient_start + direction * std::max(1.f, scale);
    const float maximum_radius = static_cast<float>(std::max(width, height));

    const PaintShaderUniforms& uniforms = shaderUniforms();

    auto pass = [&](LLRenderTarget& input, LLRenderTarget& output, float axis_x, float axis_y) {
        LLGLDisable disable_scissor(GL_SCISSOR_TEST);
        LLGLDisable disable_blend(GL_BLEND);
        gGL.flush();
        paint::RenderTargetGuard target_guard(output);
        paint::MatrixGuard matrix_guard({0.f, 0.f, capture.w, capture.h});
        mProgram.bind();
        setPaintOp(mProgram, PaintOp::Blur);
        mProgram.uniform2f(uniforms.effectTextureSize, static_cast<float>(width), static_cast<float>(height));
        mProgram.uniform2f(uniforms.effectBlurAxis, axis_x, axis_y);
        mProgram.uniform2f(uniforms.effectBlurRadii, std::min(effect.start_radius * scale, maximum_radius),
                           std::min(effect.end_radius * scale, maximum_radius));
        mProgram.uniform2f(uniforms.effectGradientStart, gradient_start.x, gradient_start.y);
        mProgram.uniform2f(uniforms.effectGradientEnd, gradient_end.x, gradient_end.y);
        const S32 texture_channel = mProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &input, false, LLTexUnit::TFO_BILINEAR);
        if (texture_channel >= 0) gGL.getTexUnit(texture_channel)->setTextureAddressMode(LLTexUnit::TAM_CLAMP);
        drawTexturedQuad({0.f, 0.f, capture.w, capture.h});
        gGL.flush();
        mProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
        setPaintOp(mProgram, PaintOp::Direct);
    };

    pass(source, horizontal_target, 1.f, 0.f);
    pass(horizontal_target, vertical_target, 0.f, 1.f);
    mClips.reapply();
    return &vertical_target;
}

void EffectRenderer::compositeEffect(LLRenderTarget& source, const Rect& capture, const Rect& destination, float radius, bool rounded_mask) {
    const Rect visible = intersectRects(capture, destination);
    if (!mProgram.mProgramObject || visible.empty() || capture.empty()) return;
    const PaintShaderUniforms& uniforms = shaderUniforms();

    const float u0 = (visible.left() - capture.left()) / capture.w;
    const float u1 = (visible.right() - capture.left()) / capture.w;
    const float v0 = (visible.bottom() - capture.bottom()) / capture.h;
    const float v1 = (visible.top() - capture.bottom()) / capture.h;
    mProgram.bind();
    setPaintOp(mProgram, PaintOp::Composite);
    mProgram.uniform4f(uniforms.effectCaptureRect, capture.x, capture.y, capture.w, capture.h);
    mProgram.uniform4f(uniforms.effectMaskRect, destination.x, destination.y, destination.w, destination.h);
    mProgram.uniform1f(uniforms.effectMaskRadius, std::max(0.f, radius));
    mProgram.uniform1i(uniforms.effectRoundedMask, rounded_mask ? 1 : 0);
    const S32 texture_channel = mProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &source, false, LLTexUnit::TFO_BILINEAR);
    if (texture_channel >= 0) gGL.getTexUnit(texture_channel)->setTextureAddressMode(LLTexUnit::TAM_CLAMP);
    drawTexturedQuad(visible, u0, v0, u1, v1);
    gGL.flush();
    mProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
    setPaintOp(mProgram, PaintOp::Direct);
}

void EffectRenderer::begin(const Rect& rect, const Style& style, float scale) {
    if (mEffectDepth == mEffectLayers.size()) mEffectLayers.emplace_back();
    EffectLayer& frame = mEffectLayers[mEffectDepth++];
    frame.layerEffects.clear();
    frame.effectRect = rect;
    frame.scale = std::max(scale, .0001f);
    frame.capture.reset();
    const float maximum_padding = std::max(mClips.bounds().w, mClips.bounds().h);

    auto captureBounds = [&](float padding) {
        const Rect expanded = snappedOutward(expandedRect(rect, padding), frame.scale);
        return intersectRects(expanded, mClips.bounds());
    };

    for (const Effect& effect : style.effects) {
        if (effect.start_radius <= 0.f && effect.end_radius <= 0.f) continue;
        if (effect.kind == EffectKind::LayerBlur) {
            frame.layerEffects.push_back(effect);
            continue;
        }
        const float padding = std::min(std::max(effect.start_radius, effect.end_radius) * 2.f + 1.f / frame.scale, maximum_padding);
        const Rect capture = captureBounds(padding);
        if (capture.empty()) continue;
        if (!captureFramebuffer(capture, frame.scale, mBackgroundTargets[0])) continue;
        LLRenderTarget* blurred = applyBlur(mBackgroundTargets[0], mBackgroundTargets[1], mBackgroundTargets[2], capture, rect, effect, frame.scale);
        compositeEffect(*blurred, capture, rect, style.border_radius, true);
    }

    if (frame.layerEffects.empty()) return;
    float padding = 1.f / frame.scale;
    for (const Effect& effect : frame.layerEffects)
        padding = std::min(padding + std::max(effect.start_radius, effect.end_radius) * 2.f, maximum_padding);
    frame.captureRect = captureBounds(padding);
    if (frame.captureRect.empty()) return;

    const U32 width = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.w * frame.scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(frame.captureRect.h * frame.scale)));
    if (!ensureTarget(frame.targets[0], width, height)) return;
    gGL.flush();
    frame.capture.emplace(mClips, frame.targets[0], frame.captureRect);
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
    compositeEffect(*source, frame.captureRect, frame.captureRect, 0.f, false);
}

void GeometryPainter::prepareVectorDraw() {
    gGL.getTexUnit(0)->unbind(LLTexUnit::TT_TEXTURE);
    if (program.mProgramObject) {
        program.bind();
        setPaintOp(program, PaintOp::Direct);
    }
}

void TextPainter::prepareTextDraw() {
    gUIProgram.bind();
}

void GeometryPainter::drawMesh(const Mesh& mesh) {
    if (mesh.empty() || !program.mProgramObject) return;
    prepareVectorDraw();
    gGL.begin(LLRender::TRIANGLES);
    for (const Vertex& vertex : mesh.triangles) {
        gGL.color4f(vertex.color.r, vertex.color.g, vertex.color.b, vertex.color.a);
        gGL.vertex2f(vertex.position.x, vertex.position.y);
    }
    gGL.end();
}

void TextPainter::paintText(const std::string& text, const Rect& rect, const Style& style) {
    if (text.empty() || style.font_size <= 0.f || style.text_color.a <= 0.f) return;
    const LLFontGL& font = fontForStyle(style);
    prepareTextDraw();
    const LLFontGL::HAlign horizontal = horizontalAlignment(style);
    constexpr LLFontGL::VAlign vertical = LLFontGL::VCENTER;
    const LLColor4 color(style.text_color.r, style.text_color.g, style.text_color.b, style.text_color.a);
    const LLFontGL::TextSpacing spacing = usedTextSpacing(style, font);
    font.renderUTF8(text, 0, textX(rect, horizontal), textY(rect, vertical), color, horizontal, vertical, LLFontGL::NORMAL, LLFontGL::NO_SHADOW,
                    S32_MAX, S32_MAX, nullptr, false, true, spacing);
    if (style.font_strike) {
        const float width = measureOpenGLText(text, style).x;
        const float anchor = textX(rect, horizontal);
        const float left = horizontal == LLFontGL::RIGHT ? anchor - width : horizontal == LLFontGL::HCENTER ? anchor - width * .5f : anchor;
        const float thickness = std::max(1.f, std::round(font.getLineHeight() / 14.f));
        const float y = textBaseline(rect, vertical, font) + font.getAscenderHeight() * .3f;
        geometry.drawRoundedShape(PaintOp::Fill, {left, y - thickness * .5f, width, thickness}, 0.f, 0.f, style.text_color);
    }
}

void GeometryPainter::drawRoundedShape(PaintOp op, const Rect& rect, float radius, float border_width, const Color& color, OutlineStyle outline_style,
                                       std::optional<TopBorderGap> top_border_gap) {
    if (!program.mProgramObject || rect.empty() || color.a <= 0.f || (op == PaintOp::Border && border_width <= 0.f)) return;
    const PaintShaderUniforms& uniforms = shaderUniforms();
    const float padding = op == PaintOp::Border ? 1.f : 0.f;
    const Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, op);
    program.uniform4f(uniforms.shapeRect, rect.x, rect.y, rect.w, rect.h);
    program.uniform1f(uniforms.shapeRadius, std::max(0.f, radius));
    program.uniform1f(uniforms.shapeBorderWidth, std::clamp(border_width, 0.f, std::min(rect.w, rect.h) * 0.5f));
    program.uniform4f(uniforms.shapeColor, color.r, color.g, color.b, color.a);
    program.uniform2f(uniforms.shapeOffset, padding, padding);
    program.uniform1i(uniforms.outlineStyle, outlineOpValue(outline_style));
    program.uniform2f(uniforms.topBorderGap, top_border_gap ? top_border_gap->left - rect.left() : -1.f,
                      top_border_gap ? top_border_gap->right - rect.left() : -1.f);
    drawShapeQuad(quad);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawRoundedGradient(const Rect& rect, float radius, const Gradient& gradient, const EdgeInsets* border_widths,
                                          std::optional<TopBorderGap> top_border_gap) {
    if (!program.mProgramObject || rect.empty() || gradient.stops.size() < 2 || gradient.stops.size() > 8 || (border_widths && !border_widths->any()))
        return;
    const PaintShaderUniforms& uniforms = shaderUniforms();

    constexpr float radians_per_degree = std::numbers::pi_v<float> / 180.f;
    const float angle = gradient.angle_degrees * radians_per_degree;
    const Vec2 direction(std::sin(angle), std::cos(angle));
    const float extent = std::abs(direction.x) * rect.w + std::abs(direction.y) * rect.h;
    const Vec2 center(rect.w * .5f, rect.h * .5f);
    const Vec2 start = center - direction * (extent * .5f);
    const Vec2 end = center + direction * (extent * .5f);
    const Vec2 gradient_center_value(gradient.center.x * rect.w, gradient.center.y * rect.h);
    const float far_x = std::max(gradient_center_value.x, rect.w - gradient_center_value.x);
    const float far_y = std::max(gradient_center_value.y, rect.h - gradient_center_value.y);
    Vec2 radial_radius;
    if (gradient.radial_shape == RadialGradientShape::Circle) {
        const float far_corner = std::sqrt(far_x * far_x + far_y * far_y);
        radial_radius = {far_corner, far_corner};
    } else {
        constexpr float square_root_two = 1.41421356237f;
        radial_radius = {far_x * square_root_two, far_y * square_root_two};
    }
    constexpr std::size_t max_gradient_stops = 8;
    std::array<GLfloat, max_gradient_stops * 4> colors{};
    std::array<GLfloat, max_gradient_stops> stops{};
    for (std::size_t index = 0; index < gradient.stops.size(); ++index) {
        const GradientStop& stop = gradient.stops[index];
        colors[index * 4] = stop.color.r;
        colors[index * 4 + 1] = stop.color.g;
        colors[index * 4 + 2] = stop.color.b;
        colors[index * 4 + 3] = stop.color.a;
        stops[index] = stop.position;
    }

    const float padding = border_widths ? 1.f : 0.f;
    const Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    program.bind();
    setPaintOp(program, border_widths ? PaintOp::GradientBorder : PaintOp::Gradient);
    program.uniform4f(uniforms.shapeRect, rect.x, rect.y, rect.w, rect.h);
    program.uniform1f(uniforms.shapeRadius, std::max(0.f, radius));
    program.uniform2f(uniforms.shapeOffset, padding, padding);
    if (border_widths) program.uniform4f(uniforms.borderWidths, border_widths->top, border_widths->right, border_widths->bottom, border_widths->left);
    program.uniform2f(uniforms.topBorderGap, top_border_gap ? top_border_gap->left - rect.left() : -1.f,
                      top_border_gap ? top_border_gap->right - rect.left() : -1.f);
    program.uniform1i(uniforms.gradientKind, gradientOpValue(gradient.kind));
    program.uniform1i(uniforms.gradientRepeating, gradient.repeating ? 1 : 0);
    program.uniform2f(uniforms.gradientStart, start.x, start.y);
    program.uniform2f(uniforms.gradientEnd, end.x, end.y);
    program.uniform2f(uniforms.gradientCenter, gradient_center_value.x, gradient_center_value.y);
    program.uniform2f(uniforms.gradientRadius, radial_radius.x, radial_radius.y);
    program.uniform1f(uniforms.gradientAngle, gradient.angle_degrees);
    program.uniform1i(uniforms.gradientStopCount, static_cast<GLint>(gradient.stops.size()));
    program.uniform4fv(uniforms.gradientColors, static_cast<U32>(gradient.stops.size()), colors.data());
    program.uniform1fv(uniforms.gradientStops, static_cast<U32>(gradient.stops.size()), stops.data());
    drawShapeQuad(quad);
    gGL.flush();
    setPaintOp(program, PaintOp::Direct);
}

void GeometryPainter::drawShadow(const Rect& rect, float radius, const BoxShadow& shadow) {
    if (!program.mProgramObject || rect.empty() || shadow.color.a <= 0.f) return;
    const PaintShaderUniforms& uniforms = shaderUniforms();

    const Rect box = snapped(rect);
    Rect shape = box;
    Rect quad = box;
    Vec2 local_shape_offset;
    PaintOp op = PaintOp::InsetShadow;
    if (!shadow.inset) {
        op = PaintOp::OuterShadow;
        shape = {box.x + shadow.horizontal - shadow.spread, box.y - shadow.vertical - shadow.spread, std::max(0.f, box.w + shadow.spread * 2.f),
                 std::max(0.f, box.h + shadow.spread * 2.f)};
        if (shape.empty()) return;
        const float padding = shadow.blur * 2.f;
        quad = {shape.x - padding, shape.y - padding, shape.w + padding * 2.f, shape.h + padding * 2.f};
        local_shape_offset = {padding, padding};
    }

    prepareVectorDraw();
    program.bind();
    setPaintOp(program, op);
    program.uniform4f(uniforms.shapeRect, shape.x, shape.y, shape.w, shape.h);
    program.uniform1f(uniforms.shapeRadius, std::max(0.f, radius + (shadow.inset ? 0.f : shadow.spread)));
    program.uniform4f(uniforms.shapeColor, shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a);
    program.uniform2f(uniforms.shapeOffset, local_shape_offset.x, local_shape_offset.y);
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

void GeometryPainter::drawBorder(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    if (!style.border_width.any()) return;
    const Rect box = snapped(rect);
    if (style.border_gradient) {
        drawRoundedGradient(box, style.border_radius, *style.border_gradient, &style.border_width, top_border_gap);
        return;
    }
    if (style.border_color.a <= 0.f) return;
    if (style.border_width.is_uniform()) {
        drawRoundedShape(PaintOp::Border, box, style.border_radius, style.border_width.top, style.border_color, OutlineStyle::Solid, top_border_gap);
        return;
    }
    const EdgeInsets& width = style.border_width;
    if (top_border_gap && !top_border_gap->empty()) {
        const float gap_left = std::clamp(top_border_gap->left, box.left(), box.right());
        const float gap_right = std::clamp(top_border_gap->right, gap_left, box.right());
        drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width.top, std::max(0.f, gap_left - box.left()), width.top}, 0.f, 0.f,
                         style.border_color);
        drawRoundedShape(PaintOp::Fill, {gap_right, box.top() - width.top, std::max(0.f, box.right() - gap_right), width.top}, 0.f, 0.f,
                         style.border_color);
    } else drawRoundedShape(PaintOp::Fill, {box.left(), box.top() - width.top, box.w, width.top}, 0.f, 0.f, style.border_color);
    drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom(), box.w, width.bottom}, 0.f, 0.f, style.border_color);
    drawRoundedShape(PaintOp::Fill, {box.left(), box.bottom() + width.bottom, width.left, box.h - width.top - width.bottom}, 0.f, 0.f,
                     style.border_color);
    drawRoundedShape(PaintOp::Fill, {box.right() - width.right, box.bottom() + width.bottom, width.right, box.h - width.top - width.bottom}, 0.f, 0.f,
                     style.border_color);
}

void GeometryPainter::drawOutline(const Rect& rect, const Style& style) {
    if (style.outline.width <= 0.f || style.outline.color.a <= 0.f) return;
    const float width = style.outline.width;
    const float expansion = width + style.outline.offset;
    const Rect box = snapped(rect);
    drawRoundedShape(PaintOp::Border, {box.x - expansion, box.y - expansion, box.w + expansion * 2.f, box.h + expansion * 2.f},
                     std::max(0.f, style.border_radius + expansion), width, style.outline.color, style.outline.style);
}

void GeometryPainter::paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    for (auto shadow = style.shadows.rbegin(); shadow != style.shadows.rend(); ++shadow)
        if (!shadow->inset) drawShadow(rect, style.border_radius, *shadow);

    Rect fill_box = snapped(rect);
    float fill_radius = style.border_radius;
    const bool bordered = hasVisibleBorder(style);
    if (bordered) {
        if (hasOpaqueBackground(style) && (!top_border_gap || top_border_gap->empty()))
            if (style.border_gradient) drawRoundedGradient(fill_box, style.border_radius, *style.border_gradient);
            else drawRoundedShape(PaintOp::Fill, fill_box, style.border_radius, 0.f, style.border_color);
        else drawBorder(rect, style, top_border_gap);
        fill_box = insetRect(fill_box, style.border_width);
        fill_radius = std::max(0.f, style.border_radius - style.border_width.max_value());
    }
    if (style.background_color.a > 0.f) drawRoundedShape(PaintOp::Fill, fill_box, fill_radius, 0.f, style.background_color);
    if (style.background_gradient) drawRoundedGradient(fill_box, fill_radius, *style.background_gradient);
    for (auto shadow = style.shadows.rbegin(); shadow != style.shadows.rend(); ++shadow)
        if (shadow->inset) drawShadow(fill_box, fill_radius, *shadow);
    if (!bordered) drawBorder(rect, style, top_border_gap);
    drawOutline(rect, style);
}

void IconPainter::paintIcon(const SvgIcon* icon, const Rect& rect, const Style& style, float scale) {
    if (!icon || icon->empty()) return;
    const Rect source = icon->view_box;
    const Color color = style.icon_stroke_color.a > 0.f ? style.icon_stroke_color : style.background_color;
    const float unit_scale = std::min(rect.w / std::max(0.0001f, source.w), rect.h / std::max(0.0001f, source.h));
    const float width = style.svg_stroke_width ? style.svg_stroke_width->pixels : icon->stroke_width * unit_scale;
    const StrokeCap cap = style.svg_stroke_cap_set ? style.svg_stroke_cap : icon->stroke_cap;

    auto draw_path = [&](const Path& source_path) {
        geometry.drawMesh(tessellateStroke(transformSvgPath(source_path, source, rect), color, width, std::max(1.f, scale), cap));
    };
    for (const Path& icon_path : icon->paths) draw_path(icon_path);
}
} // namespace rdui
