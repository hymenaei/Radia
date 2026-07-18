#ifndef LL_RDUI_NATIVE_WINDOW_H
#define LL_RDUI_NATIVE_WINDOW_H

#include "rduinativeinput.h"
#include "rduitypes.h"
#include "stdtypes.h"
#include <algorithm>
#include <cmath>
#include <memory>
#include <optional>
#include <string>

namespace rdui::viewer
{
    class NativeWindowClient
    {
        public:
            virtual ~NativeWindowClient() = default;
            virtual NativeInputDispatchResult dispatchNative(const NativeInputEvent& event) = 0;
            virtual void paintNative(S32 pixel_width, S32 pixel_height, F32 scale) = 0;
            virtual void closeNative() = 0;
            virtual void nativeDragEnded() = 0;
            virtual void nativeResizeEnded(F32 logical_width, F32 logical_height) = 0;
    };

    struct NativeRect
    {
        S32 x = 0;
        S32 y = 0;
        S32 width = 0;
        S32 height = 0;
    };

    struct NativePoint
    {
        S32 x = 0;
        S32 y = 0;
    };

    inline NativeRect nativeRectForLogicalResize(
        const NativeRect& initial, const Rect& logical, F32 scale)
    {
        const F32 safe_scale = std::max(0.25f, scale);
        return {
            initial.x + static_cast<S32>(std::round(logical.x * safe_scale)),
            initial.y + initial.height
                - static_cast<S32>(std::round((logical.y + logical.h) * safe_scale)),
            std::max(1, static_cast<S32>(std::round(logical.w * safe_scale))),
            std::max(1, static_cast<S32>(std::round(logical.h * safe_scale))),
        };
    }

    class NativeWindow
    {
        public:
            static std::unique_ptr<NativeWindow> create(
                const NativeRect& rect, const std::string& title, NativeWindowClient& client);
            static bool placementVisible(const NativeRect& rect, const std::string& monitor_id);

            virtual ~NativeWindow() = default;
            virtual void show(bool activate) = 0;
            virtual void setVisible(bool visible) = 0;
            virtual void setTitle(const std::string& title) = 0;
            virtual void pump() = 0;
            virtual void render() = 0;
            virtual void setScaleMultiplier(F32 multiplier) = 0;
            virtual void setLogicalSize(F32 width, F32 height) = 0;
            virtual void setLogicalRect(const Rect& rect) = 0;
            virtual void beginDrag(F32 logical_x, F32 logical_y,
                                   const std::optional<NativePoint>& cursor = std::nullopt) = 0;
            virtual void beginResize() = 0;
            virtual NativeRect rect() const = 0;
            virtual std::string monitorId() const = 0;
            virtual F32 scale() const = 0;
    };

    class NativeWindowFactory
    {
        public:
            virtual ~NativeWindowFactory() = default;
            virtual std::unique_ptr<NativeWindow> create(
                const NativeRect& rect, const std::string& title, NativeWindowClient& client) = 0;
            virtual bool placementVisible(const NativeRect& rect,
                                          const std::string& monitor_id) const = 0;
    };

    NativeWindowFactory& defaultNativeWindowFactory();
}

#endif // LL_RDUI_NATIVE_WINDOW_H
