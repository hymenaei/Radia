/**
 * @file resourcecompiler.h
 * @brief Builds validated Widget trees from Layout Resource documents.
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

#ifndef RD_LAYOUT_RESOURCECOMPILER_H
#define RD_LAYOUT_RESOURCECOMPILER_H

#include <cstdint>
#include <string>
#include <unordered_map>
#include "layout/document.h"
#include "widgets/widgetcontract.h"

namespace radia::ui {
class LayoutResourceCompiler final {
public:
    explicit LayoutResourceCompiler(const LayoutDocumentMap* documents = nullptr);

    LayoutBuildResult buildWidgetTreeFromResource(const std::string& resourceId, const LayoutBuildContext* context = nullptr) const;
    LayoutBuildResult buildWidgetTreeFromString(const std::string& xml, const std::string& sourceName = {},
                                                const LayoutBuildContext* context = nullptr) const;
    DiagnosticResult validateWidgetDefaults(const std::string& element, const LayoutBuildContext* context = nullptr) const;

private:
    struct BuildState;
    struct ChildBuildContext;
    enum class ChildHandling : uint8_t { Unhandled, Handled };

    static std::string normalizeResource(std::string resourceId);
    std::unique_ptr<Widget> buildDocument(const LayoutDocument& document, std::unique_ptr<Widget> root, BuildState& state) const;
    std::unique_ptr<Widget> buildNode(const LayoutNode& layoutNode, const std::string& source, std::unique_ptr<Widget> widget,
                                      BuildState& state) const;
    const WidgetContract* lookupWidgetContract(const LayoutNode& layoutNode, const std::string& source, BuildState& state) const;
    bool resolveWidgetResource(const LayoutElement& element, const WidgetContract& contract, const std::string& source,
                               std::unique_ptr<Widget>& widget, BuildState& state) const;
    void buildChildren(Widget& target, const LayoutElement& element, const WidgetContract& contract, const std::string& source,
                       BuildState& state) const;
    ChildHandling appendTextContent(const LayoutContent& content, ChildBuildContext& context) const;
    ChildHandling consumeFlowBreak(const LayoutNode& childNode, ChildBuildContext& context) const;
    ChildHandling consumeScopedInline(const LayoutNode& childNode, ChildBuildContext& context) const;
    ChildHandling consumeChildContainer(const LayoutNode& childNode, ChildBuildContext& context) const;
    ChildHandling buildRegularChild(const LayoutNode& childNode, ChildBuildContext& context) const;
    std::unique_ptr<Widget> createResourceWidget(const std::string& resourceId, BuildState& state) const;
    void loadWidgetDefaults(const std::string& element, BuildState& state) const;
    void validateWidgetScope(Widget& scope, BuildState& state, const std::string& source, bool countRoot = true) const;

    const LayoutDocumentMap* mDocuments = nullptr;
    std::unordered_map<std::string, WidgetContract> mWidgetContracts;
};
} // namespace radia::ui
#endif // RD_LAYOUT_RESOURCECOMPILER_H
