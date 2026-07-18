#ifndef LL_RDUI_FLOATER_CONTROLLER_H
#define LL_RDUI_FLOATER_CONTROLLER_H

#include "rduibinder.h"
#include "rduidiagnostic.h"

#include <string>

namespace rdui
{
    class Floater;

    namespace viewer
    {
        class FloaterController
        {
            public:
                virtual ~FloaterController() = default;

                virtual std::string resourceId() const = 0;
                virtual PreparedBindingResult prepareBindings(Floater& floater) = 0;
                virtual void commitBindings(PreparedBinding&& binding) = 0;
                virtual void idle() {}
                virtual void reportReloadSucceeded() {}
                virtual void reportReloadFailed(const DiagnosticResult&) {}
        };
    }
}

#endif // LL_RDUI_FLOATER_CONTROLLER_H
