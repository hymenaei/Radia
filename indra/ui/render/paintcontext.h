/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include "nativeappearance.h"
#include "style/style.h"
#include "text/metrics.h"

namespace radia::ui {
struct TopBorderGap {
    float left = 0.f;
    float right = 0.f;

    bool empty() const { return right <= left; }
};

class PaintContext : public TextMetrics {
public:
    virtual ~PaintContext() = default;

    virtual void beginFrame() {}
    virtual void endFrame() {}
    virtual void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) = 0;
    virtual void popClip() = 0;
    virtual void pushTranslation(const Vec2& translation) = 0;
    virtual void popTranslation() = 0;
    virtual void beginEffects(const Rect& rect, const Style& style, float scale) = 0;
    virtual void endEffects() = 0;
    virtual void paintNativeScrollbar(const NativeScrollbarPaintRequest&) {}
    virtual void paintBox(const Rect& rect, const Style& style, std::optional<TopBorderGap> topBorderGap = std::nullopt) = 0;
    virtual void paintText(const std::string& text, const Rect& rect, const Style& style) = 0;
    virtual void paintIcon(const std::string& name, const Rect& rect, const Style& style, float scale) = 0;
};
} // namespace radia::ui
