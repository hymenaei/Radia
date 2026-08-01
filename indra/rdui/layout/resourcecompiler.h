/**
 * @file resourcecompiler.h
 * @brief
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

#include <string>
#include <unordered_map>
#include "layout/document.h"
#include "widgets/widgetcontract.h"

namespace rdui {
class LayoutResourceCompiler final {
public:
    explicit LayoutResourceCompiler(const LayoutDocumentMap* documents = nullptr);

    ViewBuildResult createFromResource(const std::string& filename, const ViewBuildContext* context = nullptr) const;
    ViewBuildResult createFromString(const std::string& xml, const std::string& source_name = {}, const ViewBuildContext* context = nullptr) const;
    DiagnosticResult validateWidgetDefaults(const std::string& element, const ViewBuildContext* context = nullptr) const;

private:
    struct BuildState;
    static std::string normalizeResource(std::string filename);
    std::unique_ptr<Widget> buildDocument(const LayoutDocument& document, std::unique_ptr<Widget> root, BuildState& state) const;
    std::unique_ptr<Widget> createResourceWidget(const std::string& filename, BuildState& state) const;
    void loadWidgetDefaults(const std::string& element, BuildState& state) const;
    void validateViewScope(Widget& scope, BuildState& state, const std::string& source, bool count_root = true) const;

    const LayoutDocumentMap* mDocuments = nullptr;
    std::unordered_map<std::string, WidgetContract> mWidgetContracts;
};
} // namespace rdui
#endif // RD_LAYOUT_RESOURCECOMPILER_H
