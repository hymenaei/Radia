/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <vector>
#include "event.h"
#include "resolver.h"
#include "stdtypes.h"
#include "style/style.h"
#include "text/keybinding.h"
#include "types.h"

class LLControlGroup;
class LLGLSLShader;
class LLWindow;

namespace radia::ui {
class Document;
class FloaterElement;
class PaintContext;
class System;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::CursorStyle;
using radia::ui::Document;
using radia::ui::FloaterElement;
using radia::ui::KeybindingPresentation;
using radia::ui::KeyEvent;
using radia::ui::PaintContext;
using radia::ui::PointerEvent;
using radia::ui::WheelEvent;
using radia::ui::System;

struct InputDispatchResult {
    bool handled = false;
    std::optional<CursorStyle> cursor;
};

class DocumentController;

struct RuntimeKeybindingState {
    U64 generation = 0;
    U32 mode = 0;

    bool operator==(const RuntimeKeybindingState& other) const { return generation == other.generation && mode == other.mode; }
};

class Runtime final {
public:
    using ControllerFactory = std::function<std::unique_ptr<DocumentController>(System& system, Document& document)>;
    using KeybindingResolver = std::function<KeybindingPresentation(const std::string&)>;
    using KeybindingStateProvider = std::function<RuntimeKeybindingState()>;
    using SkinSnapshotProvider = std::function<SkinSnapshotResult()>;
    using Clock = std::function<std::chrono::steady_clock::time_point()>;
    using PaintContextFactory = std::function<std::unique_ptr<PaintContext>(LLGLSLShader&, System&)>;

    struct IntegrationHooks {
        KeybindingResolver resolveKeybinding;
        KeybindingStateProvider keybindingState;
    };

    struct TestOverrides {
        SkinSnapshotProvider captureSkin;
        Clock now;
        PaintContextFactory paintContext;
    };

    Runtime(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, LLWindow* mainWindow,
            IntegrationHooks integrationHooks, TestOverrides testOverrides = {});
    ~Runtime();
    Runtime(const Runtime&) = delete;
    Runtime& operator=(const Runtime&) = delete;

    bool initialize();
    void shutdown();
    bool registerFloater(std::string definitionId, std::string resourceId, ControllerFactory factory);
    FloaterElement* openFloater(const std::string& definitionId, const std::string& instanceKey = {});
    void restoreWorkspace();
    void endAccountSession();
    void requestSkinReload();
    void setVisibility(bool visible);
    void frame(S32 width, S32 height, F32 paintScale = 1.f, F32 paintOriginX = 0.f, F32 paintOriginY = 0.f);
    void idle();
    bool hasPointerCapture() const;
    InputDispatchResult pointerMove(const PointerEvent& event);
    InputDispatchResult pointerDown(const PointerEvent& event);
    InputDispatchResult pointerUp(const PointerEvent& event);
    void pointerLeave();
    InputDispatchResult scroll(const WheelEvent& event);
    InputDispatchResult keyDown(const KeyEvent& event);
    InputDispatchResult keyUp(const KeyEvent& event);
    InputDispatchResult character(std::uint32_t codepoint);
    void focusLost();
    void pointerCaptureLost();

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace radia::viewer::ui
