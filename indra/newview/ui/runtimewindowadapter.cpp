/**
 * @file runtimewindowadapter.cpp
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

#include "llviewerprecompiledheaders.h"
#include "runtimewindowadapter.h"
#include <algorithm>
#include <cmath>
#include "llcoord.h"
#include "llwindow.h"

namespace radia::viewer::ui {
using radia::ui::Rect;
using radia::ui::Vec2;

RuntimeWindowAdapter::RuntimeWindowAdapter(LLWindow*& window, AuxiliaryWindowFactory& auxiliaryWindows, DisplayScale displayScale, MainSize mainSize,
                                           ClearDragState clearDragState)
    : mWindow(window), mAuxiliaryWindows(auxiliaryWindows), mDisplayScale(std::move(displayScale)), mMainSize(std::move(mainSize)),
      mClearDragState(std::move(clearDragState)) {}

AuxiliaryWindowRect RuntimeWindowAdapter::mainRectToNative(const Rect& rect) const {
    if (!mWindow) return {};
    const AuxiliaryScreenPoint topLeft = mainPointToNative({rect.left(), rect.top()});
    const AuxiliaryScreenPoint bottomRight = mainPointToNative({rect.right(), rect.bottom()});
    return {std::min(topLeft.x, bottomRight.x), std::min(topLeft.y, bottomRight.y), std::abs(bottomRight.x - topLeft.x),
            std::abs(bottomRight.y - topLeft.y)};
}

AuxiliaryScreenPoint RuntimeWindowAdapter::mainPointToNative(const Vec2& point) const {
    if (!mWindow) return {};
    const Vec2 scale = mDisplayScale ? mDisplayScale() : Vec2{1.f, 1.f};
    const Vec2 scaled = detail::scaleLogicalPoint(point, scale);
    LLCoordScreen screen;
    mWindow->convertCoords(LLCoordGL(ll_round(scaled.x), ll_round(scaled.y)), &screen);
    return {screen.mX, screen.mY};
}

float RuntimeWindowAdapter::nativeScaleMultiplier() const {
    if (!mWindow) return 1.f;
    const AuxiliaryWindowRect sample = mainRectToNative({0.f, 0.f, 100.f, 100.f});
    const float effectiveScale = sample.width > 0 ? static_cast<float>(sample.width) / 100.f : 1.f;
    return effectiveScale / std::max(0.25f, mWindow->getSystemUISize());
}

Vec2 RuntimeWindowAdapter::nativeBottomLeftInMain(const AuxiliaryWindowRect& rect) const {
    if (!mWindow) return {};
    const Vec2 scale = mDisplayScale ? mDisplayScale() : Vec2{1.f, 1.f};
    LLCoordGL bottomLeft;
    mWindow->convertCoords(LLCoordScreen(rect.x, rect.y + rect.height), &bottomLeft);
    return detail::unscaleNativePoint({static_cast<float>(bottomLeft.mX), static_cast<float>(bottomLeft.mY)}, scale);
}

bool RuntimeWindowAdapter::nativePointInsideMain(const Vec2& point) const {
    if (!mWindow || !mMainSize) return false;
    const auto [width, height] = mMainSize();
    if (width <= 0 || height <= 0) return false;
    const AuxiliaryWindowRect main = mainRectToNative({0.f, 0.f, static_cast<float>(width), static_cast<float>(height)});
    return point.x >= main.x && point.x <= main.x + main.width && point.y >= main.y && point.y <= main.y + main.height;
}

bool RuntimeWindowAdapter::placementVisible(const AuxiliaryWindowRect& rect) const {
    return mAuxiliaryWindows.placementVisible(rect);
}

std::optional<AuxiliaryScreenPoint> RuntimeWindowAdapter::releasePointerForDetach(const Vec2& mainPosition) {
    const std::optional<AuxiliaryScreenPoint> screenCursor = mWindow ? std::optional<AuxiliaryScreenPoint>(mainPointToNative(mainPosition)) : std::nullopt;
    if (mClearDragState) mClearDragState();
    if (mWindow) mWindow->releaseMouse();
    return screenCursor;
}

void RuntimeWindowAdapter::setMouseClipping(bool enabled) {
    if (mWindow) mWindow->setMouseClipping(enabled);
}
} // namespace radia::viewer::ui
