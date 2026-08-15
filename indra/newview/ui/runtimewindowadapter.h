/**
 * @file runtimewindowadapter.h
 * @brief Adapts the viewer's main window to UI runtime coordinates and pointer ownership.
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

#ifndef RD_RUNTIMEWINDOWADAPTER_H
#define RD_RUNTIMEWINDOWADAPTER_H

#include <functional>
#include <optional>
#include <utility>
#include "detachedfloatermanager.h"
#include "types.h"
#include "widgets/floater.h"

class LLWindow;

namespace radia::viewer::ui {
using radia::ui::Rect;
using radia::ui::Vec2;

namespace detail {
inline Vec2 scaleLogicalPoint(const Vec2& point, const Vec2& scale) {
    return {point.x * scale.x, point.y * scale.y};
}

inline Vec2 unscaleNativePoint(const Vec2& point, const Vec2& scale) {
    return {scale.x != 0.f ? point.x / scale.x : point.x, scale.y != 0.f ? point.y / scale.y : point.y};
}
} // namespace detail

class RuntimeWindowAdapter final : public DetachedFloaterManager::DetachedFloaterEnvironment {
public:
    using DisplayScale = std::function<Vec2()>;
    using MainSize = std::function<std::pair<int, int>()>;
    using ClearDragState = std::function<void()>;

    RuntimeWindowAdapter(LLWindow*& window, AuxiliaryWindowFactory& auxiliaryWindows, DisplayScale displayScale, MainSize mainSize,
                         ClearDragState clearDragState);

    AuxiliaryWindowRect mainRectToNative(const Rect& rect) const override;
    float nativeScaleMultiplier() const override;
    Vec2 nativeBottomLeftInMain(const AuxiliaryWindowRect& rect) const override;
    bool nativePointInsideMain(const Vec2& point) const override;
    bool hasDisplaySpaceBeyondEdge(const Vec2& position, const Vec2& delta) const;
    bool placementVisible(const AuxiliaryWindowRect& rect) const override;
    std::optional<AuxiliaryScreenPoint> releasePointerForDetach(const Vec2& mainPosition) override;

    void setMouseClipping(bool enabled);

private:
    AuxiliaryScreenPoint mainPointToNative(const Vec2& point) const;

    LLWindow*& mWindow;
    AuxiliaryWindowFactory& mAuxiliaryWindows;
    DisplayScale mDisplayScale;
    MainSize mMainSize;
    ClearDragState mClearDragState;
};
} // namespace radia::viewer::ui
#endif // RD_RUNTIMEWINDOWADAPTER_H
