/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include "PaintContext.h"

class LLGLSLShader;

namespace Core {
class System;

class OpenGLPaintContext final : public PaintContext {
public:
    OpenGLPaintContext(::LLGLSLShader& shapeProgram, const System& system);
    ~OpenGLPaintContext() override;

    void beginFrame(const PaintTarget& target) override;
    void endFrame() override;
    void destroyGL() override;
    Layout::Vec2 measureText(const std::string& text, const Style::ComputedStyle& style) const override;
    float usedLetterSpacing(const Style::ComputedStyle& style) const override;
    std::uint64_t generation() const noexcept override;
    void pushClip(const Layout::Rect& rect, float scale, Layout::ClipAxes axes = Layout::ClipAxes::Both) override;
    void popClip() override;
    void pushTranslation(const Layout::Vec2& translation) override;
    void popTranslation() override;
    void beginEffects(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale) override;
    void endEffects() override;
    void paintNativeScrollbar(const NativeScrollbarPaintRequest& request) override;
    void paintNativeInput(const NativeInputPaintRequest& request) override;
    void paintNativeInputMark(const NativeInputMarkPaintRequest& request) override;
    void paintNativeButton(const NativeButtonPaintRequest& request) override;
    void paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style,
        std::optional<TopBorderGap> topBorderGap = std::nullopt) override;
    void paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style) override;

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace Core
