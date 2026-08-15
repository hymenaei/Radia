/**
 * @file widgetcontractmodel.h
 * @brief Private Widget Contract data model.
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

#ifndef RD_WIDGETS_WIDGETCONTRACTMODEL_H
#define RD_WIDGETS_WIDGETCONTRACTMODEL_H

#include <cstddef>
#include <cstdint>
#include <functional>
#include <memory>
#include <optional>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>
#include "layout/buildresult.h"
#include "text/inlinecontent.h"
#include "text/source.h"
#include "types.h"
#include "widgetevent.h"

namespace radia::ui {
class LayoutElement;
class LayoutBuildContext;
class WidgetScopeContext;

enum class WidgetTextContentMode : uint8_t { Unsupported, WidgetText, TextChildren, InlineContent };

struct CompositePartContract {
    CompositePartContract() = default;

    std::string path;
    std::string parentPath;
    std::function<std::unique_ptr<Widget>()> create;
    std::function<void(Widget&, Widget&)> bind;
    std::vector<WidgetState> producedStates;
    bool eager = true;
};

struct CompositeTopology {
    std::unordered_map<std::string, std::size_t> indices;
    std::vector<std::size_t> order;
    bool valid = true;
};

struct ScopedInlineContentContract {
    std::string elementName;
    std::vector<InlineContentKind> accepted;
    std::function<Widget*(TextSource, Widget&, LayoutBuildResult&, const std::string&, std::size_t, std::size_t)> apply;
};

struct WidgetAttributeContract {
    std::vector<std::string> names;
    std::function<void(const LayoutElement&, Widget&, LayoutBuildResult&, const std::string&, const LayoutBuildContext*)> apply;
};

struct ResourceRootContract {
    std::string expectedElementName;
};

struct ChildClaim {
    enum class Kind : uint8_t { NotHandled, Handled, Container };

    static ChildClaim notHandled() { return ChildClaim(Kind::NotHandled); }
    static ChildClaim handled() { return ChildClaim(Kind::Handled); }
    static ChildClaim routeTo(Widget& container) { return ChildClaim(Kind::Container, &container); }

    Kind kind() const { return mKind; }
    Widget* container() const { return mContainer; }

private:
    explicit ChildClaim(Kind kind, Widget* container = nullptr) : mKind(kind), mContainer(container) {}

    Kind mKind = Kind::NotHandled;
    Widget* mContainer = nullptr;
};

struct WidgetAttributeBehavior {
    std::function<void(const LayoutElement&, Widget&, LayoutBuildResult&, const std::string&, const LayoutBuildContext*)> apply;
};

struct WidgetCompositionBehavior {
    std::function<void(const LayoutElement&, Widget&, const WidgetScopeContext&, LayoutBuildResult&, const std::string&)> validate;
};

struct WidgetChildrenBehavior {
    std::unordered_map<std::string, std::vector<std::string>> partAttributes;
    std::function<ChildClaim(const LayoutElement&, Widget&, LayoutBuildResult&, const std::string&)> claim;
};

struct WidgetContentBehavior {
    WidgetTextContentMode mode = WidgetTextContentMode::Unsupported;
    std::function<std::unique_ptr<Widget>(TextSource)> createTextChild;
    std::function<void(std::string, Widget&, LayoutBuildResult&, const std::string&, const LayoutBuildContext*, std::size_t)> applyText;
    std::vector<InlineContentKind> acceptedInlineContent;
    std::function<void(TextSource, Widget&)> applyInlineContent;
    std::unordered_map<std::string, ScopedInlineContentContract> scopedInlineContent;
};

class WidgetScopeContext {
public:
    using LabelablePredicate = std::function<bool(const Widget&)>;

    WidgetScopeContext(const std::unordered_map<std::string, Widget*>& widgets, const std::unordered_set<std::string>& ambiguousIds,
                       LabelablePredicate isLabelable)
        : mWidgets(widgets), mAmbiguousIds(ambiguousIds), mIsLabelable(std::move(isLabelable)) {}

    Widget* find(const std::string& id) const {
        if (id.empty() || ambiguous(id)) return nullptr;
        const auto found = mWidgets.find(id);
        return found == mWidgets.end() ? nullptr : found->second;
    }

    bool ambiguous(const std::string& id) const { return mAmbiguousIds.find(id) != mAmbiguousIds.end(); }

    bool labelable(const Widget& widget) const { return mIsLabelable(widget); }

private:
    const std::unordered_map<std::string, Widget*>& mWidgets;
    const std::unordered_set<std::string>& mAmbiguousIds;
    LabelablePredicate mIsLabelable;
};

struct WidgetContract {
    WidgetContract() = default;

    std::string elementName;
    std::function<std::unique_ptr<Widget>()> create;
    std::vector<std::string> attributes;
    WidgetAttributeBehavior attributeBehavior;
    WidgetCompositionBehavior compositionBehavior;
    WidgetChildrenBehavior childrenBehavior;
    WidgetContentBehavior contentBehavior;
    std::optional<ResourceRootContract> resourceRoot;
    std::vector<WidgetEventKind> supportedEvents;
    std::vector<WidgetState> producedStates;
    std::vector<CompositePartContract> compositeParts;
    std::shared_ptr<const CompositeTopology> compositeTopology;
    bool labelable = false;
    bool scopedOnly = false;
};
} // namespace radia::ui
#endif // RD_WIDGETS_WIDGETCONTRACTMODEL_H
