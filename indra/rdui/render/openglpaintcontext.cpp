/**
 * @file openglpaintcontext.cpp
 * @brief
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
#include <utility>
#include <vector>
#include "llfontgl.h"
#include "llgl.h"
#include "llglslshader.h"
#include "llrender.h"
#include "llrendertarget.h"
#include "llshadermgr.h"
#include "llstring.h"
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

void applyScissor(const Rect& rect, float scale, const Vec2& render_origin, const Vec2& pixel_origin) {
    const S32 left = llfloor(pixel_origin.x + (rect.left() - render_origin.x) * scale);
    const S32 right = llceil(pixel_origin.x + (rect.right() - render_origin.x) * scale);
    const S32 bottom = llfloor(pixel_origin.y + (rect.bottom() - render_origin.y) * scale);
    const S32 top = llceil(pixel_origin.y + (rect.top() - render_origin.y) * scale);
    glScissor(left, bottom, llmax(0, right - left), llmax(0, top - bottom));
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
} // namespace

struct OpenGLPaintContext::Impl {
    struct RenderState {
        Vec2 origin;
        Vec2 pixelOrigin;
        Rect bounds;
    };

    struct EffectFrame {
        std::array<LLRenderTarget, 3> targets;
        std::vector<Effect> layerEffects;
        RenderState previousState;
        Rect effectRect;
        Rect captureRect;
        float radius = 0.f;
        float scale = 1.f;
        LLRender::eMatrixMode previousMatrixMode = LLRender::MM_MODELVIEW;
        bool capturing = false;
    };

    std::unique_ptr<LLGLSUIDefault> uiState;
    std::unique_ptr<LLGLState> scissorState;
    std::vector<std::pair<Rect, float>> clips;
    std::array<LLRenderTarget, 3> backgroundTargets;
    std::vector<std::unique_ptr<EffectFrame>> effectFrames;
    std::size_t effectDepth = 0;
    RenderState renderState;
    GLint previousScissor[4] = {};
};

OpenGLPaintContext::OpenGLPaintContext(::LLGLSLShader& shape_program, const System& system)
    : mShapeProgram(shape_program), mSystem(system), mImpl(std::make_unique<Impl>()) {}

OpenGLPaintContext::~OpenGLPaintContext() = default;

Vec2 OpenGLPaintContext::measureText(const std::string& text, const Style& style) const {
    return measureOpenGLText(text, style);
}

float OpenGLPaintContext::usedLetterSpacing(const Style& style) const {
    if (style.font_size <= 0.f) return 0.f;
    const LLFontGL& font = fontForStyle(style);
    return usedTextSpacing(style, font).letter;
}

void OpenGLPaintContext::beginFrame() {
    mImpl->clips.clear();
    mImpl->effectDepth = 0;
    GLint viewport[4]{};
    glGetIntegerv(GL_VIEWPORT, viewport);
    mImpl->renderState = {{0.f, 0.f},
                          {static_cast<float>(viewport[0]), static_cast<float>(viewport[1])},
                          {0.f, 0.f, static_cast<float>(viewport[2]), static_cast<float>(viewport[3])}};
    mImpl->uiState = std::make_unique<LLGLSUIDefault>();
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
}

void OpenGLPaintContext::endFrame() {
    llassert(mImpl->effectDepth == 0);
    while (!mImpl->clips.empty()) popClip();
    gUIProgram.bind();
    mImpl->uiState.reset();
}

void OpenGLPaintContext::pushClip(const Rect& rect, float scale, ClipAxes axes) {
    const float resolved_scale = std::max(0.f, scale);
    const Rect inherited = mImpl->clips.empty() ? mImpl->renderState.bounds : mImpl->clips.back().first;
    const Rect clipped = clipToAxes(inherited, rect, axes);
    if (mImpl->clips.empty()) {
        gGL.flush();
        glGetIntegerv(GL_SCISSOR_BOX, mImpl->previousScissor);
        mImpl->scissorState = std::make_unique<LLGLState>(GL_SCISSOR_TEST, LLGLState::ENABLED_STATE);
    }
    mImpl->clips.emplace_back(clipped, resolved_scale);
    gGL.flush();
    applyScissor(intersectRects(clipped, mImpl->renderState.bounds), resolved_scale, mImpl->renderState.origin, mImpl->renderState.pixelOrigin);
}

void OpenGLPaintContext::popClip() {
    if (mImpl->clips.empty()) return;
    gGL.flush();
    mImpl->clips.pop_back();
    if (!mImpl->clips.empty()) {
        const Rect& clip = mImpl->clips.back().first;
        const float scale = mImpl->clips.back().second;
        applyScissor(intersectRects(clip, mImpl->renderState.bounds), scale, mImpl->renderState.origin, mImpl->renderState.pixelOrigin);
        return;
    }
    glScissor(mImpl->previousScissor[0], mImpl->previousScissor[1], mImpl->previousScissor[2], mImpl->previousScissor[3]);
    mImpl->scissorState.reset();
}

void OpenGLPaintContext::reapplyClip() {
    if (mImpl->clips.empty()) return;
    const Rect clipped = intersectRects(mImpl->clips.back().first, mImpl->renderState.bounds);
    applyScissor(clipped, mImpl->clips.back().second, mImpl->renderState.origin, mImpl->renderState.pixelOrigin);
}

void OpenGLPaintContext::pushEffectMatrices(const Rect& bounds) {
    gGL.matrixMode(LLRender::MM_PROJECTION);
    gGL.pushMatrix();
    gGL.loadIdentity();
    gGL.ortho(0.f, bounds.w, 0.f, bounds.h, -1.f, 1.f);
    gGL.matrixMode(LLRender::MM_MODELVIEW);
    gGL.pushMatrix();
    gGL.loadIdentity();
    gGL.pushUIMatrix();
    gGL.loadUIIdentity();
    gGL.translateUI(-bounds.x, -bounds.y, 0.f);
}

void OpenGLPaintContext::popEffectMatrices() {
    gGL.popUIMatrix();
    gGL.matrixMode(LLRender::MM_MODELVIEW);
    gGL.popMatrix();
    gGL.matrixMode(LLRender::MM_PROJECTION);
    gGL.popMatrix();
}

bool OpenGLPaintContext::captureFramebuffer(const Rect& capture, float scale, LLRenderTarget& target) {
    const U32 width = static_cast<U32>(std::max(1.f, std::round(capture.w * scale)));
    const U32 height = static_cast<U32>(std::max(1.f, std::round(capture.h * scale)));
    if (!ensureTarget(target, width, height)) return false;

    const S32 source_x = ll_round(mImpl->renderState.pixelOrigin.x + (capture.x - mImpl->renderState.origin.x) * scale);
    const S32 source_y = ll_round(mImpl->renderState.pixelOrigin.y + (capture.y - mImpl->renderState.origin.y) * scale);
    gGL.flush();
    target.bindTexture(0, 0, LLTexUnit::TFO_BILINEAR);
    gGL.getTexUnit(0)->setTextureAddressMode(LLTexUnit::TAM_CLAMP);
    glCopyTexSubImage2D(GL_TEXTURE_2D, 0, 0, 0, source_x, source_y, static_cast<GLsizei>(width), static_cast<GLsizei>(height));
    return true;
}

LLRenderTarget* OpenGLPaintContext::applyBlur(LLRenderTarget& source, LLRenderTarget& horizontal_target, LLRenderTarget& vertical_target,
                                              const Rect& capture, const Rect& effect_rect, const Effect& effect, float scale) {
    if (!mShapeProgram.mProgramObject) return &source;
    const U32 width = source.getWidth();
    const U32 height = source.getHeight();
    if (!ensureTarget(horizontal_target, width, height) || !ensureTarget(vertical_target, width, height)) return &source;

    constexpr float radians_per_degree = 0.0174532925199f;
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

    static LLStaticHashedString shape_mode("rduiShapeMode");
    static LLStaticHashedString texture_size("rduiEffectTextureSize");
    static LLStaticHashedString blur_axis("rduiEffectBlurAxis");
    static LLStaticHashedString blur_radii("rduiEffectBlurRadii");
    static LLStaticHashedString gradient_start_uniform("rduiEffectGradientStart");
    static LLStaticHashedString gradient_end_uniform("rduiEffectGradientEnd");

    auto pass = [&](LLRenderTarget& input, LLRenderTarget& output, float axis_x, float axis_y) {
        LLGLDisable disable_scissor(GL_SCISSOR_TEST);
        LLGLDisable disable_blend(GL_BLEND);
        gGL.flush();
        output.bindTarget();
        const LLRender::eMatrixMode previous_mode = gGL.getMatrixMode();
        pushEffectMatrices({0.f, 0.f, capture.w, capture.h});
        mShapeProgram.bind();
        mShapeProgram.uniform1i(shape_mode, 7);
        mShapeProgram.uniform2f(texture_size, static_cast<float>(width), static_cast<float>(height));
        mShapeProgram.uniform2f(blur_axis, axis_x, axis_y);
        mShapeProgram.uniform2f(blur_radii, std::min(effect.start_radius * scale, maximum_radius),
                                std::min(effect.end_radius * scale, maximum_radius));
        mShapeProgram.uniform2f(gradient_start_uniform, gradient_start.x, gradient_start.y);
        mShapeProgram.uniform2f(gradient_end_uniform, gradient_end.x, gradient_end.y);
        const S32 texture_channel = mShapeProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &input, false, LLTexUnit::TFO_BILINEAR);
        if (texture_channel >= 0) gGL.getTexUnit(texture_channel)->setTextureAddressMode(LLTexUnit::TAM_CLAMP);
        drawTexturedQuad({0.f, 0.f, capture.w, capture.h});
        gGL.flush();
        mShapeProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
        mShapeProgram.uniform1i(shape_mode, 0);
        popEffectMatrices();
        gGL.matrixMode(previous_mode);
        output.flush();
    };

    pass(source, horizontal_target, 1.f, 0.f);
    pass(horizontal_target, vertical_target, 0.f, 1.f);
    reapplyClip();
    return &vertical_target;
}

void OpenGLPaintContext::compositeEffect(LLRenderTarget& source, const Rect& capture, const Rect& destination, float radius, bool rounded_mask) {
    const Rect visible = intersectRects(capture, destination);
    if (!mShapeProgram.mProgramObject || visible.empty() || capture.empty()) return;
    static LLStaticHashedString shape_mode("rduiShapeMode");
    static LLStaticHashedString capture_rect("rduiEffectCaptureRect");
    static LLStaticHashedString mask_rect("rduiEffectMaskRect");
    static LLStaticHashedString mask_radius("rduiEffectMaskRadius");
    static LLStaticHashedString rounded("rduiEffectRoundedMask");

    const float u0 = (visible.left() - capture.left()) / capture.w;
    const float u1 = (visible.right() - capture.left()) / capture.w;
    const float v0 = (visible.bottom() - capture.bottom()) / capture.h;
    const float v1 = (visible.top() - capture.bottom()) / capture.h;
    mShapeProgram.bind();
    mShapeProgram.uniform1i(shape_mode, 8);
    mShapeProgram.uniform4f(capture_rect, capture.x, capture.y, capture.w, capture.h);
    mShapeProgram.uniform4f(mask_rect, destination.x, destination.y, destination.w, destination.h);
    mShapeProgram.uniform1f(mask_radius, std::max(0.f, radius));
    mShapeProgram.uniform1i(rounded, rounded_mask ? 1 : 0);
    const S32 texture_channel = mShapeProgram.bindTexture(LLShaderMgr::DIFFUSE_MAP, &source, false, LLTexUnit::TFO_BILINEAR);
    if (texture_channel >= 0) gGL.getTexUnit(texture_channel)->setTextureAddressMode(LLTexUnit::TAM_CLAMP);
    drawTexturedQuad(visible, u0, v0, u1, v1);
    gGL.flush();
    mShapeProgram.unbindTexture(LLShaderMgr::DIFFUSE_MAP);
    mShapeProgram.uniform1i(shape_mode, 0);
}

void OpenGLPaintContext::beginEffects(const Rect& rect, const Style& style, float scale) {
    if (mImpl->effectDepth == mImpl->effectFrames.size()) mImpl->effectFrames.push_back(std::make_unique<Impl::EffectFrame>());
    Impl::EffectFrame& frame = *mImpl->effectFrames[mImpl->effectDepth++];
    frame.layerEffects.clear();
    frame.effectRect = rect;
    frame.radius = style.border_radius;
    frame.scale = std::max(scale, .0001f);
    frame.capturing = false;
    const float maximum_padding = std::max(mImpl->renderState.bounds.w, mImpl->renderState.bounds.h);

    auto captureBounds = [&](float padding) {
        const Rect expanded = snappedOutward(expandedRect(rect, padding), frame.scale);
        return intersectRects(expanded, mImpl->renderState.bounds);
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
        if (!captureFramebuffer(capture, frame.scale, mImpl->backgroundTargets[0])) continue;
        LLRenderTarget* blurred =
            applyBlur(mImpl->backgroundTargets[0], mImpl->backgroundTargets[1], mImpl->backgroundTargets[2], capture, rect, effect, frame.scale);
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
    frame.previousState = mImpl->renderState;
    frame.previousMatrixMode = gGL.getMatrixMode();
    gGL.flush();
    frame.targets[0].bindTarget();
    {
        LLGLDisable disable_scissor(GL_SCISSOR_TEST);
        GLfloat clear_color[4]{};
        glGetFloatv(GL_COLOR_CLEAR_VALUE, clear_color);
        glClearColor(0.f, 0.f, 0.f, 0.f);
        frame.targets[0].clear(GL_COLOR_BUFFER_BIT);
        glClearColor(clear_color[0], clear_color[1], clear_color[2], clear_color[3]);
    }
    pushEffectMatrices(frame.captureRect);
    mImpl->renderState = {{frame.captureRect.x, frame.captureRect.y}, {0.f, 0.f}, frame.captureRect};
    reapplyClip();
    gGL.blendFunc(LLRender::BF_SOURCE_ALPHA, LLRender::BF_ONE_MINUS_SOURCE_ALPHA, LLRender::BF_ONE, LLRender::BF_ONE_MINUS_SOURCE_ALPHA);
    frame.capturing = true;
}

void OpenGLPaintContext::endEffects() {
    llassert(mImpl->effectDepth > 0);
    Impl::EffectFrame& frame = *mImpl->effectFrames[--mImpl->effectDepth];
    if (!frame.capturing) return;

    gGL.flush();
    popEffectMatrices();
    gGL.matrixMode(frame.previousMatrixMode);
    frame.targets[0].flush();
    mImpl->renderState = frame.previousState;
    reapplyClip();

    LLRenderTarget* source = &frame.targets[0];
    for (const Effect& effect : frame.layerEffects) {
        LLRenderTarget& horizontal = frame.targets[1];
        LLRenderTarget& vertical = source == &frame.targets[0] ? frame.targets[2] : frame.targets[0];
        source = applyBlur(*source, horizontal, vertical, frame.captureRect, frame.effectRect, effect, frame.scale);
    }
    compositeEffect(*source, frame.captureRect, frame.captureRect, 0.f, false);
    frame.capturing = false;
}

void OpenGLPaintContext::prepareVectorDraw() {
    gGL.getTexUnit(0)->unbind(LLTexUnit::TT_TEXTURE);
    static LLStaticHashedString mode("rduiShapeMode");
    if (mShapeProgram.mProgramObject) {
        mShapeProgram.bind();
        mShapeProgram.uniform1i(mode, 0);
    }
}

void OpenGLPaintContext::prepareTextDraw() {
    gUIProgram.bind();
}

void OpenGLPaintContext::drawMesh(const Mesh& mesh) {
    if (mesh.empty() || !mShapeProgram.mProgramObject) return;
    prepareVectorDraw();
    gGL.begin(LLRender::TRIANGLES);
    for (const Vertex& vertex : mesh.triangles) {
        gGL.color4f(vertex.color.r, vertex.color.g, vertex.color.b, vertex.color.a);
        gGL.vertex2f(vertex.position.x, vertex.position.y);
    }
    gGL.end();
}

void OpenGLPaintContext::paintText(const std::string& text, const Rect& rect, const Style& style) {
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
        drawRoundedShape(1, {left, y - thickness * .5f, width, thickness}, 0.f, 0.f, style.text_color);
    }
}

void OpenGLPaintContext::drawRoundedShape(int mode, const Rect& rect, float radius, float border_width, const Color& color,
                                          OutlineStyle outline_style, std::optional<TopBorderGap> top_border_gap) {
    if (!mShapeProgram.mProgramObject || rect.empty() || color.a <= 0.f || (mode == 2 && border_width <= 0.f)) return;
    static LLStaticHashedString shape_mode("rduiShapeMode");
    static LLStaticHashedString shape_rect("rduiShapeRect");
    static LLStaticHashedString shape_radius("rduiShapeRadius");
    static LLStaticHashedString shape_border("rduiShapeBorderWidth");
    static LLStaticHashedString shape_color("rduiShapeColor");
    static LLStaticHashedString shape_offset("rduiShapeOffset");
    static LLStaticHashedString shape_outline_style("rduiOutlineStyle");
    static LLStaticHashedString border_gap("rduiTopBorderGap");
    const float padding = mode == 2 ? 1.f : 0.f;
    const Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    mShapeProgram.bind();
    mShapeProgram.uniform1i(shape_mode, mode);
    mShapeProgram.uniform4f(shape_rect, rect.x, rect.y, rect.w, rect.h);
    mShapeProgram.uniform1f(shape_radius, std::max(0.f, radius));
    mShapeProgram.uniform1f(shape_border, std::clamp(border_width, 0.f, std::min(rect.w, rect.h) * 0.5f));
    mShapeProgram.uniform4f(shape_color, color.r, color.g, color.b, color.a);
    mShapeProgram.uniform2f(shape_offset, padding, padding);
    mShapeProgram.uniform1i(shape_outline_style, static_cast<GLint>(outline_style));
    mShapeProgram.uniform2f(border_gap, top_border_gap ? top_border_gap->left - rect.left() : -1.f,
                            top_border_gap ? top_border_gap->right - rect.left() : -1.f);
    drawShapeQuad(quad);
    gGL.flush();
    mShapeProgram.uniform1i(shape_mode, 0);
}

void OpenGLPaintContext::drawRoundedGradient(const Rect& rect, float radius, const Gradient& gradient, const EdgeInsets* border_widths,
                                             std::optional<TopBorderGap> top_border_gap) {
    if (!mShapeProgram.mProgramObject
        || rect.empty()
        || gradient.stops.size() < 2
        || gradient.stops.size() > 8
        || (border_widths && !border_widths->any()))
        return;
    static LLStaticHashedString shape_mode("rduiShapeMode");
    static LLStaticHashedString shape_rect("rduiShapeRect");
    static LLStaticHashedString shape_radius("rduiShapeRadius");
    static LLStaticHashedString shape_offset("rduiShapeOffset");
    static LLStaticHashedString border_edges("rduiBorderWidths");
    static LLStaticHashedString gradient_kind("rduiGradientKind");
    static LLStaticHashedString gradient_repeating("rduiGradientRepeating");
    static LLStaticHashedString gradient_start("rduiGradientStart");
    static LLStaticHashedString gradient_end("rduiGradientEnd");
    static LLStaticHashedString gradient_center("rduiGradientCenter");
    static LLStaticHashedString gradient_radius("rduiGradientRadius");
    static LLStaticHashedString gradient_angle("rduiGradientAngle");
    static LLStaticHashedString gradient_count("rduiGradientStopCount");
    static LLStaticHashedString gradient_colors("rduiGradientColors");
    static LLStaticHashedString gradient_stops("rduiGradientStops");
    static LLStaticHashedString border_gap("rduiTopBorderGap");

    constexpr float radians_per_degree = 0.0174532925199f;
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
    std::vector<GLfloat> colors;
    std::vector<GLfloat> stops;
    colors.reserve(gradient.stops.size() * 4);
    stops.reserve(gradient.stops.size());
    for (const GradientStop& stop : gradient.stops) {
        colors.insert(colors.end(), {stop.color.r, stop.color.g, stop.color.b, stop.color.a});
        stops.push_back(stop.position);
    }

    const float padding = border_widths ? 1.f : 0.f;
    const Rect quad = {rect.x - padding, rect.y - padding, rect.w + padding * 2.f, rect.h + padding * 2.f};
    prepareVectorDraw();
    mShapeProgram.bind();
    mShapeProgram.uniform1i(shape_mode, border_widths ? 6 : 3);
    mShapeProgram.uniform4f(shape_rect, rect.x, rect.y, rect.w, rect.h);
    mShapeProgram.uniform1f(shape_radius, std::max(0.f, radius));
    mShapeProgram.uniform2f(shape_offset, padding, padding);
    if (border_widths) mShapeProgram.uniform4f(border_edges, border_widths->top, border_widths->right, border_widths->bottom, border_widths->left);
    mShapeProgram.uniform2f(border_gap, top_border_gap ? top_border_gap->left - rect.left() : -1.f,
                            top_border_gap ? top_border_gap->right - rect.left() : -1.f);
    mShapeProgram.uniform1i(gradient_kind, static_cast<GLint>(gradient.kind));
    mShapeProgram.uniform1i(gradient_repeating, gradient.repeating ? 1 : 0);
    mShapeProgram.uniform2f(gradient_start, start.x, start.y);
    mShapeProgram.uniform2f(gradient_end, end.x, end.y);
    mShapeProgram.uniform2f(gradient_center, gradient_center_value.x, gradient_center_value.y);
    mShapeProgram.uniform2f(gradient_radius, radial_radius.x, radial_radius.y);
    mShapeProgram.uniform1f(gradient_angle, gradient.angle_degrees);
    mShapeProgram.uniform1i(gradient_count, static_cast<GLint>(gradient.stops.size()));
    mShapeProgram.uniform4fv(gradient_colors, static_cast<U32>(gradient.stops.size()), colors.data());
    mShapeProgram.uniform1fv(gradient_stops, static_cast<U32>(gradient.stops.size()), stops.data());
    drawShapeQuad(quad);
    gGL.flush();
    mShapeProgram.uniform1i(shape_mode, 0);
}

void OpenGLPaintContext::drawShadow(const Rect& rect, float radius, const BoxShadow& shadow) {
    if (!mShapeProgram.mProgramObject || rect.empty() || shadow.color.a <= 0.f) return;
    static LLStaticHashedString shape_mode("rduiShapeMode");
    static LLStaticHashedString shape_rect("rduiShapeRect");
    static LLStaticHashedString shape_radius("rduiShapeRadius");
    static LLStaticHashedString shape_color("rduiShapeColor");
    static LLStaticHashedString shape_offset("rduiShapeOffset");
    static LLStaticHashedString shadow_offset("rduiShadowOffset");
    static LLStaticHashedString shadow_blur("rduiShadowBlur");
    static LLStaticHashedString shadow_spread("rduiShadowSpread");

    const Rect box = snapped(rect);
    Rect shape = box;
    Rect quad = box;
    Vec2 local_shape_offset;
    int mode = 5;
    if (!shadow.inset) {
        mode = 4;
        shape = {box.x + shadow.horizontal - shadow.spread, box.y - shadow.vertical - shadow.spread, std::max(0.f, box.w + shadow.spread * 2.f),
                 std::max(0.f, box.h + shadow.spread * 2.f)};
        if (shape.empty()) return;
        const float padding = shadow.blur * 2.f;
        quad = {shape.x - padding, shape.y - padding, shape.w + padding * 2.f, shape.h + padding * 2.f};
        local_shape_offset = {padding, padding};
    }

    prepareVectorDraw();
    mShapeProgram.bind();
    mShapeProgram.uniform1i(shape_mode, mode);
    mShapeProgram.uniform4f(shape_rect, shape.x, shape.y, shape.w, shape.h);
    mShapeProgram.uniform1f(shape_radius, std::max(0.f, radius + (shadow.inset ? 0.f : shadow.spread)));
    mShapeProgram.uniform4f(shape_color, shadow.color.r, shadow.color.g, shadow.color.b, shadow.color.a);
    mShapeProgram.uniform2f(shape_offset, local_shape_offset.x, local_shape_offset.y);
    mShapeProgram.uniform2f(shadow_offset, shadow.horizontal, -shadow.vertical);
    mShapeProgram.uniform1f(shadow_blur, shadow.blur);
    mShapeProgram.uniform1f(shadow_spread, shadow.spread);
    drawShapeQuad(quad);
    gGL.flush();
    mShapeProgram.uniform1i(shape_mode, 0);
}

void OpenGLPaintContext::drawShapeQuad(const Rect& rect) {
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

void OpenGLPaintContext::drawBorder(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    if (!style.border_width.any()) return;
    const Rect box = snapped(rect);
    if (style.border_gradient) {
        drawRoundedGradient(box, style.border_radius, *style.border_gradient, &style.border_width, top_border_gap);
        return;
    }
    if (style.border_color.a <= 0.f) return;
    if (style.border_width.is_uniform()) {
        drawRoundedShape(2, box, style.border_radius, style.border_width.top, style.border_color, OutlineStyle::Solid, top_border_gap);
        return;
    }
    const EdgeInsets& width = style.border_width;
    if (top_border_gap && !top_border_gap->empty()) {
        const float gap_left = std::clamp(top_border_gap->left, box.left(), box.right());
        const float gap_right = std::clamp(top_border_gap->right, gap_left, box.right());
        drawRoundedShape(1, {box.left(), box.top() - width.top, std::max(0.f, gap_left - box.left()), width.top}, 0.f, 0.f, style.border_color);
        drawRoundedShape(1, {gap_right, box.top() - width.top, std::max(0.f, box.right() - gap_right), width.top}, 0.f, 0.f, style.border_color);
    } else drawRoundedShape(1, {box.left(), box.top() - width.top, box.w, width.top}, 0.f, 0.f, style.border_color);
    drawRoundedShape(1, {box.left(), box.bottom(), box.w, width.bottom}, 0.f, 0.f, style.border_color);
    drawRoundedShape(1, {box.left(), box.bottom() + width.bottom, width.left, box.h - width.top - width.bottom}, 0.f, 0.f, style.border_color);
    drawRoundedShape(1, {box.right() - width.right, box.bottom() + width.bottom, width.right, box.h - width.top - width.bottom}, 0.f, 0.f,
                     style.border_color);
}

void OpenGLPaintContext::drawOutline(const Rect& rect, const Style& style) {
    if (style.outline.width <= 0.f || style.outline.color.a <= 0.f) return;
    const float width = style.outline.width;
    const float expansion = width + style.outline.offset;
    const Rect box = snapped(rect);
    drawRoundedShape(2, {box.x - expansion, box.y - expansion, box.w + expansion * 2.f, box.h + expansion * 2.f},
                     std::max(0.f, style.border_radius + expansion), width, style.outline.color, style.outline.style);
}

void OpenGLPaintContext::paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> top_border_gap) {
    for (auto shadow = style.shadows.rbegin(); shadow != style.shadows.rend(); ++shadow)
        if (!shadow->inset) drawShadow(rect, style.border_radius, *shadow);

    Rect fill_box = snapped(rect);
    float fill_radius = style.border_radius;
    const bool bordered = hasVisibleBorder(style);
    if (bordered) {
        if (hasOpaqueBackground(style) && (!top_border_gap || top_border_gap->empty()))
            if (style.border_gradient) drawRoundedGradient(fill_box, style.border_radius, *style.border_gradient);
            else drawRoundedShape(1, fill_box, style.border_radius, 0.f, style.border_color);
        else drawBorder(rect, style, top_border_gap);
        fill_box = insetRect(fill_box, style.border_width);
        fill_radius = std::max(0.f, style.border_radius - style.border_width.max_value());
    }
    if (style.background_color.a > 0.f) drawRoundedShape(1, fill_box, fill_radius, 0.f, style.background_color);
    if (style.background_gradient) drawRoundedGradient(fill_box, fill_radius, *style.background_gradient);
    for (auto shadow = style.shadows.rbegin(); shadow != style.shadows.rend(); ++shadow)
        if (shadow->inset) drawShadow(fill_box, fill_radius, *shadow);
    if (!bordered) drawBorder(rect, style, top_border_gap);
    drawOutline(rect, style);
}

void OpenGLPaintContext::paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) {
    const SvgIcon* icon = mSystem.icon(name);
    if (!icon || icon->empty()) return;
    const Rect source = icon->view_box;
    const Color color = style.icon_stroke_color.a > 0.f ? style.icon_stroke_color : style.background_color;
    const float unit_scale = std::min(rect.w / std::max(0.0001f, source.w), rect.h / std::max(0.0001f, source.h));
    const float width = style.svg_stroke_width ? style.svg_stroke_width->pixels : icon->stroke_width * unit_scale;
    const StrokeCap cap = style.svg_stroke_cap_set ? style.svg_stroke_cap : icon->stroke_cap;

    auto draw_path = [&](const Path& source_path) {
        drawMesh(tessellateStroke(transformSvgPath(source_path, source, rect), color, width, std::max(1.f, scale), cap));
    };
    for (const Path& icon_path : icon->paths) draw_path(icon_path);
}
} // namespace rdui
