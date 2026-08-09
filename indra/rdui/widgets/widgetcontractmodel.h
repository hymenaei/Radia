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
#include "action.h"
#include "layout/viewresult.h"
#include "text/inlinecontent.h"
#include "text/source.h"
#include "types.h"

namespace rdui {
class LayoutElement;
class ViewBuildContext;
class ViewScopeContext;

enum class ViewTextContent : uint8_t { Unsupported, Widget, Children, Inline };

struct CompositePartContract {
    CompositePartContract() = default;

    std::string path;
    std::string parent_path;
    std::function<std::unique_ptr<Widget>()> create;
    std::function<void(Widget&, Widget&)> bind;
    std::vector<WidgetState> produced_states;
    bool eager = true;
};

struct CompositeTopology {
    std::unordered_map<std::string, std::size_t> indices;
    std::vector<std::size_t> order;
    bool valid = true;
};

struct ScopedInlineContentContract {
    std::string element;
    std::vector<InlineContentKind> accepted;
    std::function<Widget*(TextSource, Widget&, ViewBuildResult&, const std::string&, std::size_t, std::size_t)> apply;
};

struct WidgetAttributeContract {
    std::vector<std::string> names;
    std::function<void(const LayoutElement&, Widget&, ViewBuildResult&, const std::string&, const ViewBuildContext*)> apply;
};

struct ResourceRootContract {
    std::string expected_element;
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
    std::function<void(const LayoutElement&, Widget&, ViewBuildResult&, const std::string&, const ViewBuildContext*)> apply;
};

struct WidgetCompositionBehavior {
    std::function<void(const LayoutElement&, Widget&, const ViewScopeContext&, ViewBuildResult&, const std::string&)> validate;
};

struct WidgetChildrenBehavior {
    std::unordered_map<std::string, std::vector<std::string>> part_attributes;
    std::function<ChildClaim(const LayoutElement&, Widget&, ViewBuildResult&, const std::string&)> claim;
};

struct WidgetContentBehavior {
    ViewTextContent mode = ViewTextContent::Unsupported;
    std::function<std::unique_ptr<Widget>(TextSource)> create_text_child;
    std::function<void(std::string, Widget&, ViewBuildResult&, const std::string&, const ViewBuildContext*, std::size_t)> apply_text;
    std::vector<InlineContentKind> accepted_inline_content;
    std::function<void(TextSource, Widget&)> apply_inline_content;
    std::unordered_map<std::string, ScopedInlineContentContract> scoped_inline_content;
};

class ViewScopeContext {
public:
    using LabelablePredicate = std::function<bool(const Widget&)>;

    ViewScopeContext(const std::unordered_map<std::string, Widget*>& widgets, const std::unordered_set<std::string>& ambiguous_ids,
                     LabelablePredicate is_labelable)
        : mWidgets(widgets), mAmbiguousIds(ambiguous_ids), mIsLabelable(std::move(is_labelable)) {}

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

    std::string element;
    std::function<std::unique_ptr<Widget>()> create;
    std::vector<std::string> attributes;
    WidgetAttributeBehavior attribute_behavior;
    WidgetCompositionBehavior composition_behavior;
    WidgetChildrenBehavior children_behavior;
    WidgetContentBehavior content_behavior;
    std::optional<ResourceRootContract> resource_root;
    std::vector<ActionEventKind> supported_actions;
    std::vector<WidgetState> produced_states;
    std::vector<CompositePartContract> composite_parts;
    std::shared_ptr<const CompositeTopology> composite_topology;
    bool labelable = false;
    bool scoped_only = false;
};
} // namespace rdui
#endif // RD_WIDGETS_WIDGETCONTRACTMODEL_H
