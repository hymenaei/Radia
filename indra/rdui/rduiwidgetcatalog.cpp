#include "linden_common.h"
#include "rduiwidgetcatalog.h"
#include "rduischema.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdfield.h"
#include "rdfieldset.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include "rdtext.h"

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
            add(detail::fieldContract());
            add(detail::hintContract());
            add(detail::errorContract());
            add(detail::fieldsetContract());
            add(detail::legendContract());
            add(detail::floaterContract());
            add(detail::iconContract());
            add(detail::labelContract());
            add(detail::panelContract());
            add(detail::switchContract());
            add(detail::textContract());
            return result;
        }();
        return contracts;
    }
}
