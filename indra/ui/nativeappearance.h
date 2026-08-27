/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include "style/style.h"
#include "surface/scrollgeometry.h"

namespace radia::ui {
struct NativeScrollbarMetrics {
    float thickness = 15.f;
    float minimumThumbLength = 20.f;
    float arrowLength = 15.f;
    float thumbPadding = 3.f;
};

struct NativeScrollbarState {
    ScrollbarPart hoveredPart = ScrollbarPart::NoneValue;
    ScrollbarPart pressedPart = ScrollbarPart::NoneValue;
    bool disabled = false;
};

struct NativeScrollbarClip {
    bool enabled = false;
    Rect borderBox;
    BorderRadii borderRadius;
    EdgeInsets borderWidth;
};

struct NativeScrollbarPaintRequest {
    ScrollGeometry geometry;
    NativeScrollbarMetrics metrics;
    ScrollbarColors colors;
    NativeScrollbarClip clip;
    ScrollbarMode mode = ScrollbarMode::Classic;
    LayoutDirection direction = LayoutDirection::LeftToRight;
    NativeScrollbarState horizontal;
    NativeScrollbarState vertical;
    float scale = 1.f;
    std::uint64_t appearanceRevision = 1;
};

class NativeAppearance {
public:
    virtual ~NativeAppearance() = default;
    virtual NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const = 0;
    virtual std::uint64_t revision() const noexcept = 0;
};

class FallbackNativeAppearance final : public NativeAppearance {
public:
    explicit FallbackNativeAppearance(NativeScrollbarMetrics metrics = {}) : mMetrics(metrics) {}

    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode) const override { return mMetrics; }
    std::uint64_t revision() const noexcept override { return 1; }

private:
    NativeScrollbarMetrics mMetrics;
};
} // namespace radia::ui
