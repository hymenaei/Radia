/**
 * @file auxiliarywindow.h
 * @brief Defines the platform seam for non-primary viewer windows.
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

#ifndef RD_AUXILIARYWINDOW_H
#define RD_AUXILIARYWINDOW_H

#include <algorithm>
#include <cmath>
#include <memory>
#include <optional>
#include <string>
#include "llcursortypes.h"
#include "llkeyboard.h"
#include "stdtypes.h"

struct AuxiliaryWindowRect {
    S32 x = 0;
    S32 y = 0;
    S32 width = 0;
    S32 height = 0;
};

struct AuxiliaryWindowPoint {
    S32 x = 0;
    S32 y = 0;
};

struct AuxiliaryLogicalRect {
    F32 x = 0.f;
    F32 y = 0.f;
    F32 width = 0.f;
    F32 height = 0.f;
};

inline AuxiliaryWindowRect auxiliaryWindowRectForLogicalResize(const AuxiliaryWindowRect& initial, const AuxiliaryLogicalRect& logical, F32 scale) {
    const F32 safeScale = std::max(0.25f, scale);
    return {
        initial.x + static_cast<S32>(std::round(logical.x * safeScale)),
        initial.y + initial.height - static_cast<S32>(std::round((logical.y + logical.height) * safeScale)),
        std::max(1, static_cast<S32>(std::round(logical.width * safeScale))),
        std::max(1, static_cast<S32>(std::round(logical.height * safeScale))),
    };
}

enum class AuxiliaryPointerButton : U8 { NoButton, Left, Right, Middle, Auxiliary1, Auxiliary2 };
enum class AuxiliaryInteractionLoss : U8 { Focus, Capture };

struct AuxiliaryInputResult {
    bool handled = false;
    std::optional<ECursorType> cursor;
};

class AuxiliaryWindowClient {
public:
    virtual ~AuxiliaryWindowClient() = default;

    virtual AuxiliaryInputResult pointerMove(F32 x, F32 y, AuxiliaryPointerButton button, MASK modifiers, U8 clickCount, F32 deltaX, F32 deltaY) = 0;
    virtual AuxiliaryInputResult pointerDown(F32 x, F32 y, AuxiliaryPointerButton button, MASK modifiers, U8 clickCount, F32 deltaX, F32 deltaY) = 0;
    virtual AuxiliaryInputResult pointerUp(F32 x, F32 y, AuxiliaryPointerButton button, MASK modifiers, U8 clickCount, F32 deltaX, F32 deltaY) = 0;
    virtual void pointerLeave() = 0;
    virtual AuxiliaryInputResult scroll(S32 x, S32 y, F32 horizontal, F32 vertical, MASK modifiers) = 0;
    virtual AuxiliaryInputResult keyDown(KEY key, MASK modifiers, bool repeated) = 0;
    virtual AuxiliaryInputResult keyUp(KEY key, MASK modifiers) = 0;
    virtual AuxiliaryInputResult character(U32 codepoint, MASK modifiers) = 0;
    virtual void interactionLost(AuxiliaryInteractionLoss loss) = 0;

    virtual void paint(S32 pixelWidth, S32 pixelHeight, F32 scale) = 0;
    virtual void closeRequested() = 0;
    virtual void dragEnded() = 0;
    virtual void resizeEnded(F32 logicalWidth, F32 logicalHeight) = 0;
};

class AuxiliaryWindow {
public:
    virtual ~AuxiliaryWindow() = default;
    virtual void show(bool activate) = 0;
    virtual void setVisible(bool visible) = 0;
    virtual void setTitle(const std::string& title) = 0;
    virtual void pump() = 0;
    virtual void render() = 0;
    virtual void setScaleMultiplier(F32 multiplier) = 0;
    virtual void setLogicalSize(F32 width, F32 height) = 0;
    virtual void setLogicalRect(const AuxiliaryLogicalRect& rect) = 0;
    virtual void beginDrag(F32 logicalX, F32 logicalY, const std::optional<AuxiliaryWindowPoint>& cursor = std::nullopt) = 0;
    virtual void beginResize() = 0;
    virtual AuxiliaryWindowRect rect() const = 0;
    virtual F32 scale() const = 0;
};

class AuxiliaryWindowFactory {
public:
    virtual ~AuxiliaryWindowFactory() = default;
    virtual std::unique_ptr<AuxiliaryWindow> create(const AuxiliaryWindowRect& rect, const std::string& title, AuxiliaryWindowClient& client) = 0;
    virtual bool placementVisible(const AuxiliaryWindowRect& rect) const = 0;
};

AuxiliaryWindowFactory& defaultAuxiliaryWindowFactory();
#endif // RD_AUXILIARYWINDOW_H
