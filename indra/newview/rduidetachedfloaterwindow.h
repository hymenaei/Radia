/**
 * @file rduidetachedfloaterwindow.h
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

#ifndef LL_RDUI_DETACHED_FLOATER_WINDOW_H
#define LL_RDUI_DETACHED_FLOATER_WINDOW_H

#include <memory>
#include "rduidetachedfloatermanager.h"

namespace rdui {
class Floater;
class System;
} // namespace rdui

namespace rdui::viewer {
class NativeWindowFactory;

class DetachedFloaterWindow final : public DetachedFloaterPresentation {
public:
    DetachedFloaterWindow(NativeWindowFactory& native_windows, System& system, DetachedFloaterManager& manager, std::unique_ptr<Floater> floater);
    ~DetachedFloaterWindow() override;

    DetachedFloaterWindow(const DetachedFloaterWindow&) = delete;
    DetachedFloaterWindow& operator=(const DetachedFloaterWindow&) = delete;

    bool open(const NativeRect& rect, float scale_multiplier, const std::optional<Vec2>& drag_offset,
              const std::optional<Vec2>& logical_size = std::nullopt, const std::optional<NativePoint>& drag_cursor = std::nullopt) override;
    bool beginResize() override;
    void applyResize(const Rect& logical_rect) override;
    void tick() override;
    void setVisible(bool visible) override;
    std::unique_ptr<Floater> releaseFloater() override;
    Floater& replaceFloater(std::unique_ptr<Floater> replacement, const std::optional<Vec2>& logical_size) override;
    bool closeRequested() const override;
    bool minimizeRequested() const override;
    bool takeDragEnded() override;
    bool takeResizeEnded() override;
    Floater* floater() const override;
    NativeRect nativeRect() const override;
    std::string monitorId() const override;
    Vec2 logicalSize() const override;
    Vec2 headerCenterScreen() const override;

private:
    class Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace rdui::viewer
#endif // LL_RDUI_DETACHED_FLOATER_WINDOW_H
