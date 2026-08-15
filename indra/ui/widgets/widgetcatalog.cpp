/**
 * @file widgetcatalog.cpp
 * @brief Registers and validates the native Widget Contract catalog.
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

#include "linden_common.h"
#include "widgets/widgetcatalog.h"
#include "layout/schema.h"
#include "widgets/button.h"
#include "widgets/field.h"
#include "widgets/fieldset.h"
#include "widgets/floater.h"
#include "widgets/icon.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/text.h"
#include "widgets/widgetcontract.h"

namespace radia::ui {
const std::unordered_map<std::string, WidgetContract>& builtInWidgetContracts() {
    static const std::unordered_map<std::string, WidgetContract> sContracts = [] {
        std::unordered_map<std::string, WidgetContract> result;
        auto add = [&result](WidgetContract contract) {
            detail::prepareCompositeTopology(contract);
            const std::string key = schemaNameKey(contract.elementName);
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
    return sContracts;
}
} // namespace radia::ui
