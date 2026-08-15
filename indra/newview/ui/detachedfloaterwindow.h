/**
 * @file detachedfloaterwindow.h
 * @brief Presents a detached Floater in a native window and bridges its lifecycle and input.
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

#ifndef RD_DETACHEDFLOATERWINDOW_H
#define RD_DETACHEDFLOATERWINDOW_H

#include <chrono>
#include <functional>
#include <memory>
#include "detachedfloatermanager.h"

namespace radia::ui {
class Floater;
class System;
} // namespace radia::ui

class LLGLSLShader;

namespace radia::viewer::ui {
using namespace ::radia::ui;
class DetachedFloaterWindow final : public DetachedFloaterPresentation {
public:
    using Clock = std::function<std::chrono::steady_clock::time_point()>;

    DetachedFloaterWindow(::AuxiliaryWindowFactory& auxiliaryWindowFactory, LLGLSLShader& uiShader, System& system, DetachedFloaterManager& manager,
                          std::unique_ptr<Floater> floater, Clock now = {});
    ~DetachedFloaterWindow() override;

    DetachedFloaterWindow(const DetachedFloaterWindow&) = delete;
    DetachedFloaterWindow& operator=(const DetachedFloaterWindow&) = delete;

    std::optional<DetachedFloaterPresentationUpdate> open(const DetachedFloaterPresentationOpenRequest& request) override;
    bool beginResize() override;
    void applyResize(const Rect& logicalRect) override;
    DetachedFloaterPresentationUpdate update() override;
    void setVisible(bool visible) override;
    std::optional<Rect> prepareReplacement(Floater& replacement) override;
    std::unique_ptr<Floater> releaseFloater() override;
    std::unique_ptr<Floater> replaceFloater(std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logicalSize) override;

private:
    class Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace radia::viewer::ui
#endif // RD_DETACHEDFLOATERWINDOW_H
