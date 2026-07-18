#ifndef LL_RDUI_DETACHED_FLOATER_WINDOW_H
#define LL_RDUI_DETACHED_FLOATER_WINDOW_H

#include "rduidetachedfloatermanager.h"

#include <memory>

namespace rdui
{
    class Floater;
    class System;
}

namespace rdui::viewer
{
    class NativeWindowFactory;

    class DetachedFloaterWindow final : public DetachedFloaterPresentation
    {
        public:
            DetachedFloaterWindow(NativeWindowFactory& native_windows,
                                  System& system,
                                  DetachedFloaterManager& manager,
                                  std::unique_ptr<Floater> floater);
            ~DetachedFloaterWindow() override;

            DetachedFloaterWindow(const DetachedFloaterWindow&) = delete;
            DetachedFloaterWindow& operator=(const DetachedFloaterWindow&) = delete;

            bool open(const NativeRect& rect, float scale_multiplier,
                      const std::optional<Vec2>& drag_offset,
                      const std::optional<Vec2>& logical_size = std::nullopt,
                      const std::optional<NativePoint>& drag_cursor = std::nullopt) override;
            bool beginResize() override;
            void applyResize(const Rect& logical_rect) override;
            void tick() override;
            void setVisible(bool visible) override;
            std::unique_ptr<Floater> releaseFloater() override;
            Floater& replaceFloater(std::unique_ptr<Floater> replacement,
                                    const std::optional<Vec2>& logical_size) override;
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
}

#endif // LL_RDUI_DETACHED_FLOATER_WINDOW_H
