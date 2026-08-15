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
#include "widgets/floater.h"

class LLWindow;

namespace radia::viewer::ui {
using namespace ::radia::ui;

namespace detail {
inline radia::ui::Vec2 scaleLogicalPoint(const radia::ui::Vec2& point, const radia::ui::Vec2& scale) {
    return {point.x * scale.x, point.y * scale.y};
}

inline radia::ui::Vec2 unscaleNativePoint(const radia::ui::Vec2& point, const radia::ui::Vec2& scale) {
    return {scale.x != 0.f ? point.x / scale.x : point.x, scale.y != 0.f ? point.y / scale.y : point.y};
}
} // namespace detail

class RuntimeWindowAdapter final : public DetachedFloaterManager::DetachedFloaterEnvironment {
public:
    using DisplayScale = std::function<radia::ui::Vec2()>;
    using MainSize = std::function<std::pair<int, int>()>;
    using ClearDragState = std::function<void()>;

    RuntimeWindowAdapter(LLWindow*& window, AuxiliaryWindowFactory& auxiliaryWindows, DisplayScale displayScale, MainSize mainSize,
                         ClearDragState clearDragState);

    AuxiliaryWindowRect mainRectToNative(const radia::ui::Rect& rect) const override;
    float nativeScaleMultiplier() const override;
    radia::ui::Vec2 nativeBottomLeftInMain(const AuxiliaryWindowRect& rect) const override;
    bool nativePointInsideMain(const radia::ui::Vec2& point) const override;
    bool placementVisible(const AuxiliaryWindowRect& rect) const override;
    std::optional<AuxiliaryWindowPoint> releasePointerForDetach(const radia::ui::Vec2& mainPosition) override;

    void setMouseClipping(bool enabled);

private:
    AuxiliaryWindowPoint mainPointToNative(const radia::ui::Vec2& point) const;

    LLWindow*& mWindow;
    AuxiliaryWindowFactory& mAuxiliaryWindows;
    DisplayScale mDisplayScale;
    MainSize mMainSize;
    ClearDragState mClearDragState;
};
} // namespace radia::viewer::ui
#endif // RD_RUNTIMEWINDOWADAPTER_H
