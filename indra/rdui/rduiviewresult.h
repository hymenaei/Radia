#ifndef LL_RDUI_VIEW_RESULT_H
#define LL_RDUI_VIEW_RESULT_H

#include "rduidiagnostic.h"
#include "rduiwidget.h"
#include <memory>

namespace rdui
{
    struct ViewBuildResult : DiagnosticResult
    {
        std::unique_ptr<Widget> root;
        bool ok() const { return !hasErrors() && root != nullptr; }

        template<typename WidgetT>
        WidgetT* rootAs() { return dynamic_cast<WidgetT*>(root.get()); }

        template<typename WidgetT>
        const WidgetT* rootAs() const { return dynamic_cast<const WidgetT*>(root.get()); }
    };
}

#endif // LL_RDUI_VIEW_RESULT_H
