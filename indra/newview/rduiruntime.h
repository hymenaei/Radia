#ifndef LL_RDUI_RUNTIME_H
#define LL_RDUI_RUNTIME_H

#include "rduinativeinput.h"
#include "stdtypes.h"
#include <functional>
#include <memory>
#include <string>

namespace rdui
{
    class Floater;
    class System;
}

namespace rdui::viewer
{
    class FloaterController;

    class Runtime final
    {
        public:
            using ControllerFactory =
                std::function<std::unique_ptr<FloaterController>(System& system)>;

            Runtime();
            ~Runtime();
            Runtime(const Runtime&) = delete;
            Runtime& operator=(const Runtime&) = delete;

            bool initialize();
            bool registerFloater(std::string definition_id, ControllerFactory factory);
            Floater* openFloater(const std::string& definition_id,
                                 const std::string& instance_key = {});
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
}

#endif // LL_RDUI_RUNTIME_H
