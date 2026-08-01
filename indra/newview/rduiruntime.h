/**
 * @file rduiruntime.h
 * @brief
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

#ifndef LL_RDUI_RUNTIME_H
#define LL_RDUI_RUNTIME_H

#include <functional>
#include <memory>
#include <string>
#include "rduinativeinput.h"
#include "stdtypes.h"

namespace rdui {
class Floater;
class System;
} // namespace rdui

namespace rdui::viewer {
class FloaterController;

class Runtime final {
public:
    using ControllerFactory = std::function<std::unique_ptr<FloaterController>(System& system)>;

    Runtime();
    ~Runtime();
    Runtime(const Runtime&) = delete;
    Runtime& operator=(const Runtime&) = delete;

    bool initialize();
    bool registerFloater(std::string definition_id, ControllerFactory factory);
    Floater* openFloater(const std::string& definition_id, const std::string& instance_key = {});
    void restoreOpenFloaters();
    void requestReload();
    void setVisibility(bool attached_visible, bool detached_visible);
    void frame(S32 width, S32 height);
    void idle();
    bool hasPointerCapture() const;
    NativeInputDispatchResult dispatch(const NativeInputEvent& event);

private:
    struct Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui::viewer
#endif // LL_RDUI_RUNTIME_H
