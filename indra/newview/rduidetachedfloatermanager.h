#ifndef LL_RDUI_DETACHED_FLOATER_MANAGER_H
#define LL_RDUI_DETACHED_FLOATER_MANAGER_H

#include "rduifloaterplacementstore.h"
#include "rduinativewindow.h"
#include "rduitypes.h"
#include <functional>
#include <memory>
#include <optional>
#include <string>

namespace rdui
{
    class Floater;
    class Surface;
}

namespace rdui::viewer
{
    class DetachedFloaterPresentation
    {
        public:
            virtual ~DetachedFloaterPresentation() = default;

            virtual bool open(const NativeRect& rect, float scale_multiplier,
                              const std::optional<Vec2>& drag_offset,
                              const std::optional<Vec2>& logical_size = std::nullopt,
                              const std::optional<NativePoint>& drag_cursor = std::nullopt) = 0;
            virtual bool beginResize() = 0;
            virtual void applyResize(const Rect& logical_rect) = 0;
            virtual void tick() = 0;
            virtual void setVisible(bool visible) = 0;
            virtual std::unique_ptr<Floater> releaseFloater() = 0;
            virtual Floater& replaceFloater(std::unique_ptr<Floater> replacement,
                                            const std::optional<Vec2>& logical_size) = 0;
            virtual bool closeRequested() const = 0;
            virtual bool minimizeRequested() const = 0;
            virtual bool takeDragEnded() = 0;
            virtual bool takeResizeEnded() = 0;
            virtual Floater* floater() const = 0;
            virtual NativeRect nativeRect() const = 0;
            virtual std::string monitorId() const = 0;
            virtual Vec2 logicalSize() const = 0;
            virtual Vec2 headerCenterScreen() const = 0;
    };

    class DetachedFloaterManager
    {
        public:
            class Environment
            {
                public:
                    virtual ~Environment() = default;
                    virtual NativeRect mainRectToNative(const Rect& rect) const = 0;
                    virtual float nativeScaleMultiplier() const = 0;
                    virtual Vec2 nativeBottomLeftInMain(const NativeRect& rect) const = 0;
                    virtual bool nativePointInsideMain(const Vec2& point) const = 0;
                    virtual bool placementVisible(const NativeRect& rect,
                                                  const std::string& monitor_id) const = 0;
                    virtual std::optional<NativePoint> releasePointerForDetach(
                        const Vec2& main_position) = 0;
            };

            using PresentationFactory = std::function<std::unique_ptr<DetachedFloaterPresentation>(
                std::unique_ptr<Floater>&)>;

            DetachedFloaterManager(Surface& attached_surface,
                                   FloaterPlacementStore& placement_store,
                                   PresentationFactory presentation_factory,
                                   Environment& environment);
            ~DetachedFloaterManager();
            DetachedFloaterManager(const DetachedFloaterManager&) = delete;
            DetachedFloaterManager& operator=(const DetachedFloaterManager&) = delete;

            void requestDetach(const FloaterInstanceId& identity, Floater& floater,
                               const Vec2& desired, const Vec2& drag_offset);
            void processPendingDetach();
            void update();
            void setVisible(bool visible);
            bool beginResize(Floater& floater);
            void applyResize(Floater& floater, const Rect& logical_rect);

            bool contains(const Floater& floater) const;
            Vec2 logicalSize(const Floater& floater) const;
            Floater* replace(Floater& current, std::unique_ptr<Floater> replacement,
                             const std::optional<Vec2>& logical_size);
            bool restore(const FloaterInstanceId& identity, Floater& floater,
                         const DetachedFloaterPlacement& placement);
            void savePlacement(const Floater& floater);

        private:
            struct Impl;
            std::unique_ptr<Impl> mImpl;
    };
}

#endif // LL_RDUI_DETACHED_FLOATER_MANAGER_H
