/**
 * @file widgetcontract.h
 * @brief Implements Widget Contract validation and composite topology.
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

#ifndef RD_WIDGETS_WIDGETCONTRACT_H
#define RD_WIDGETS_WIDGETCONTRACT_H

#include <cstdint>
#include <functional>
#include <optional>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <utility>
#include <vector>
#include "diagnostic.h"
#include "layout/buildresult.h"
#include "layout/document.h"
#include "layout/schema.h"
#include "localization/localization.h"
#include "text/inlinecontent.h"
#include "text/source.h"
#include "widgets/widgetcontractmodel.h"

namespace radia::ui {
class Label;

class LayoutBuildContext {
public:
    LayoutBuildContext(const LocalizationCatalog& localization, std::string locale) : mLocalization(localization), mLocale(std::move(locale)) {}

    bool hasLocalizationKey(const std::string& key) const { return mLocalization.containsDefaultString(key); }

    std::string resolveText(const std::string& key) const { return mLocalization.get(mLocale, key); }

    InlineContent resolveContent(const LocalizationRequest& request) const { return mLocalization.resolve(mLocale, request); }

    TextSource localizeContent(std::string key) const {
        LocalizationRequest request = LocalizationRequest::text(std::move(key));
        InlineContent content = resolveContent(request);
        return TextSource::fromLocalization(std::move(request), std::move(content));
    }

private:
    const LocalizationCatalog& mLocalization;
    std::string mLocale;
};

bool readLayoutAttribute(const LayoutElement& element, const char* name, std::string& value);
bool readLayoutBoolean(const LayoutElement& element, const char* name, bool& value, LayoutBuildResult& result, const std::string& source);
const WidgetContract* findWidgetContract(const std::string& elementName);
const CompositePartContract* findCompositePartContract(const WidgetContract& widget, const std::vector<std::string>& parts);
bool producesState(const WidgetContract& widget, WidgetState state);
bool producesState(const CompositePartContract& part, WidgetState state);
const char* eventAttribute(WidgetEventKind kind);
namespace detail {
class WidgetCompilerAccess {
public:
    static void setStyleIdentity(Widget& widget, std::string elementName, std::string part);
    static void setIdScopeRoot(Widget& widget);
    static void setState(Widget& widget, WidgetState state, bool enabled);
    static const std::string& labelTargetId(const Label& label);
    static Widget* labelTarget(const Label& label);
    static void setLabelTarget(Label& label, Widget* target);
    static void setFlowBreakBefore(Widget& widget, bool enabled);
};

void prepareCompositeTopology(WidgetContract& contract);
void instantiateCompositeParts(Widget& owner, const WidgetContract& contract);
Widget* instantiateCompositePart(Widget& owner, const WidgetContract& contract, const std::string& path);
} // namespace detail

TextSource localizedLayoutText(std::string value, LayoutBuildResult& result, const std::string& source, const LayoutBuildContext* context,
                               std::size_t line = 0);
void validateWidgetAttributes(const LayoutElement& element, const std::vector<std::string>& widgetAttributes, LayoutBuildResult& result,
                              const std::string& source);
void applyCommonWidgetAttributes(const LayoutElement& element, Widget& widget, LayoutBuildResult& result, const std::string& source,
                                 const std::vector<WidgetEventKind>& supportedEvents = {});
} // namespace radia::ui
#endif // RD_WIDGETS_WIDGETCONTRACT_H
