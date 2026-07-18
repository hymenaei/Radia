#include "linden_common.h"
#include "rduiwidgetcatalog.h"
#include "rduischema.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdfield.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"

namespace rdui
{
    const std::unordered_map<std::string, WidgetContract>& builtInWidgetContracts()
    {
        static const std::unordered_map<std::string, WidgetContract> contracts = []
        {
            std::unordered_map<std::string, WidgetContract> result;
            auto add = [&result](WidgetContract contract)
            {
                const std::string key = schemaNameKey(contract.element);
                result.emplace(key, std::move(contract));
            };
            add(detail::buttonContract());
            add(detail::contentContract());
            add(detail::descriptionContract());
            add(detail::fieldContract());
            add(detail::floaterContract());
            add(detail::iconContract());
            add(detail::labelContract());
            add(detail::panelContract());
            add(detail::switchContract());
            return result;
        }();
        return contracts;
    }
}
