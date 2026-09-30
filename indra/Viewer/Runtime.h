/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/ComputedStyle.h>
#include <Core/Event.h>
#include <Core/KeybindingPresentation.h>
#include <chrono>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <vector>
#include "SkinSnapshot.h"
#include "stdtypes.h"

class LLControlGroup;
class LLGLSLShader;
class LLWindow;

namespace Core {
class Document;
class HTMLFloaterElement;
class PaintContext;
class System;
} // namespace Core

namespace Viewer {
struct InputDispatchResult {
    bool handled = false;
};

class DocumentController;

struct RuntimeKeybindingState {
    U64 generation = 0;
    U32 mode = 0;

    bool operator==(const RuntimeKeybindingState& other) const { return generation == other.generation && mode == other.mode; }
};

enum class RuntimeState {
    Running,
    ShuttingDown,
    Stopped,
    TeardownFailed
};

class Runtime final {
public:
    using ControllerFactory = std::function<std::unique_ptr<DocumentController>(Core::System& system, Core::Document& document)>;
    using KeybindingResolver = std::function<Core::KeybindingPresentation(const std::string&)>;
    using KeybindingStateProvider = std::function<RuntimeKeybindingState()>;
    using SkinSnapshotProvider = std::function<SkinSnapshotResult()>;
    using Clock = std::function<std::chrono::steady_clock::time_point()>;
    using PaintContextFactory = std::function<std::unique_ptr<Core::PaintContext>(LLGLSLShader&, Core::System&)>;

    struct IntegrationHooks {
        KeybindingResolver resolveKeybinding;
        KeybindingStateProvider keybindingState;
    };

    struct TestOverrides {
        SkinSnapshotProvider captureSkin;
        Clock now;
        PaintContextFactory paintContext;
        std::function<bool()> failTeardown;
    };

    Runtime(LLControlGroup& savedSettings, LLControlGroup& perAccountSettings, LLGLSLShader& uiShader, LLWindow* mainWindow,
        IntegrationHooks integrationHooks, TestOverrides testOverrides = {});
    ~Runtime();
    Runtime(const Runtime&) = delete;
    Runtime& operator=(const Runtime&) = delete;

    bool initialize();
    void shutdown();
    RuntimeState lifecycleState() const;
    bool registerFloater(std::string definitionId, std::string resource, ControllerFactory factory);
    Core::HTMLFloaterElement* openFloater(const std::string& definitionId, const std::string& instanceKey = {});
    void restoreWorkspace();
    void endAccountSession();
    void requestSkinReload();
    void setVisibility(bool visible);
    void frame(S32 width, S32 height, F32 paintScale = 1.f, F32 paintOriginX = 0.f, F32 paintOriginY = 0.f);
    void destroyGL();
    void idle();
    bool hasPointerCapture() const;
    std::optional<Core::Style::CursorStyle> pointerCursor();
    std::optional<Core::Style::CursorValue> pointerCursorValue();
    const std::string* resourceData(std::string_view reference) const;
    std::uint64_t generation() const;
    InputDispatchResult pointerMove(const Core::PointerEvent& event);
    InputDispatchResult pointerDown(const Core::PointerEvent& event);
    InputDispatchResult pointerUp(const Core::PointerEvent& event);
    void pointerLeave();
    InputDispatchResult scroll(const Core::WheelEvent& event);
    InputDispatchResult keyDown(const Core::KeyEvent& event);
    InputDispatchResult keyUp(const Core::KeyEvent& event);
    InputDispatchResult character(std::uint32_t codepoint);
    void focusLost();
    void pointerCaptureLost();

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace Viewer
