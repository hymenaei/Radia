/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include <string>
#include "render/paintcontext.h"

class LLGLSLShader;

namespace radia::ui {
class System;

class OpenGLPaintContext final : public PaintContext {
public:
    OpenGLPaintContext(::LLGLSLShader& shapeProgram, const System& system);
    ~OpenGLPaintContext() override;

    void beginFrame(const PaintTarget& target) override;
    void endFrame() override;
    Vec2 measureText(const std::string& text, const Style& style) const override;
    float usedLetterSpacing(const Style& style) const override;
    std::uint64_t generation() const override;
    void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) override;
    void popClip() override;
    void pushTranslation(const Vec2& translation) override;
    void popTranslation() override;
    void beginEffects(const Rect& rect, const Style& style, float scale) override;
    void endEffects() override;
    void paintNativeScrollbar(const NativeScrollbarPaintRequest& request) override;
    void paintNativeInput(const NativeInputPaintRequest& request) override;
    void paintNativeInputMark(const NativeInputMarkPaintRequest& request) override;
    void paintNativeButton(const NativeButtonPaintRequest& request) override;
    void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> topBorderGap = std::nullopt) override;
    void paintText(const std::string& text, const Rect& rect, const Style& style) override;
    void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) override;

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace radia::ui
