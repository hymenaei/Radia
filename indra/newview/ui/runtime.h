/**
 * @file runtime.h
 * @brief Owns the viewer-side UI runtime, Floater lifecycle, reloads, and input dispatch.
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

#ifndef RD_RUNTIME_H
#define RD_RUNTIME_H

#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include "event.h"
#include "skin/resolver.h"
#include "stdtypes.h"
#include "style/style.h"
#include "text/inlinecontent.h"
#include "types.h"

class LLControlGroup;
class LLGLSLShader;
class LLWindow;

namespace radia::ui {
class Floater;
class PaintContext;
class System;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::CursorStyle;
using radia::ui::Floater;
using radia::ui::KeybindingPresentation;
using radia::ui::KeyEvent;
using radia::ui::PaintContext;
using radia::ui::PointerEvent;
using radia::ui::ScrollEvent;
using radia::ui::System;

struct InputDispatchResult {
    bool handled = false;
    std::optional<CursorStyle> cursor;
};

class ComponentController;

struct RuntimeKeybindingState {
    U64 generation = 0;
    U32 mode = 0;

    bool operator==(const RuntimeKeybindingState& other) const { return generation == other.generation && mode == other.mode; }
};

class Runtime final {
public:
    using ControllerFactory = std::function<std::unique_ptr<ComponentController>(System& system)>;
    using KeybindingResolver = std::function<KeybindingPresentation(const std::string&)>;
    using KeybindingStateProvider = std::function<RuntimeKeybindingState()>;
    using SkinSnapshotProvider = std::function<SkinSnapshotResult()>;
    using Clock = std::function<std::chrono::steady_clock::time_point()>;
    using PaintContextFactory = std::function<std::unique_ptr<PaintContext>(LLGLSLShader&, System&)>;

    struct IntegrationHooks {
        KeybindingResolver resolveKeybinding;
        KeybindingStateProvider keybindingState;
        SkinSnapshotProvider captureSkin = {};
        Clock now = {};
        PaintContextFactory paintContext = {};
    };

    Runtime(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, LLWindow* mainWindow,
            IntegrationHooks hooks);
    ~Runtime();
    Runtime(const Runtime&) = delete;
    Runtime& operator=(const Runtime&) = delete;

    bool initialize();
    void shutdown();
    bool registerFloater(std::string definitionId, std::string resourceId, ControllerFactory factory);
    Floater* openFloater(const std::string& definitionId, const std::string& instanceKey = {});
    void restoreWorkspace();
    void endAccountSession();
    void requestSkinReload();
    void setVisibility(bool visible);
    void frame(S32 width, S32 height);
    void idle();
    bool hasPointerCapture() const;
    InputDispatchResult pointerMove(const PointerEvent& event);
    InputDispatchResult pointerDown(const PointerEvent& event);
    InputDispatchResult pointerUp(const PointerEvent& event);
    void pointerLeave();
    InputDispatchResult scroll(const ScrollEvent& event);
    InputDispatchResult keyDown(const KeyEvent& event);
    InputDispatchResult keyUp(const KeyEvent& event);
    InputDispatchResult character(std::uint32_t codepoint);
    void focusLost();
    void mouseCaptureLost();

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace radia::viewer::ui
#endif // RD_RUNTIME_H
