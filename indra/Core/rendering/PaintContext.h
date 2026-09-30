/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include <utility>
#include "ComputedStyle.h"
#include "NativeAppearance.h"
#include "PaintTarget.h"
#include "TextMeasurer.h"

namespace Core {
struct TopBorderGap {
    float left = 0.f;
    float right = 0.f;

    bool empty() const { return right <= left; }
};

struct BackgroundPaintContext {
    Layout::Vec2 localScrollTranslation;
    Layout::Vec2 paintTranslation;
    Layout::Rect viewport;
    std::optional<Layout::Rect> scrollport;
};

class PaintContext : public TextMeasurer, public NativeControlPaintContext {
public:
    virtual ~PaintContext() = default;
    virtual const TextMeasurer& textMetrics() const { return *this; }

    virtual void beginFrame(const PaintTarget&) {}
    virtual void endFrame() {}
    virtual void destroyGL() {}
    virtual void pushClip(const Layout::Rect& rect, float scale, Layout::ClipAxes axes = Layout::ClipAxes::Both) = 0;
    virtual void popClip() = 0;
    virtual void pushTranslation(const Layout::Vec2& translation) = 0;
    virtual void popTranslation() = 0;
    virtual void beginEffects(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale) = 0;
    virtual void endEffects() = 0;
    virtual void paintNativeScrollbar(const NativeScrollbarPaintRequest&) {}
    virtual void paintNativeInput(const NativeInputPaintRequest&) {}
    void paintNativeBox(const Layout::Rect& rect, const Style::ComputedStyle& style) override { paintBox(rect, style); }
    void paintNativeInputMark(const NativeInputMarkPaintRequest&) override {}
    virtual void paintNativeButton(const NativeButtonPaintRequest&) {}
    virtual void paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style,
        std::optional<TopBorderGap> topBorderGap = std::nullopt) = 0;
    virtual void paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style) = 0;
    void setBackgroundPaintContext(std::optional<BackgroundPaintContext> context) { mBackgroundPaintContext = std::move(context); }
    const std::optional<BackgroundPaintContext>& backgroundPaintContext() const { return mBackgroundPaintContext; }

private:
    std::optional<BackgroundPaintContext> mBackgroundPaintContext;
};
} // namespace Core
