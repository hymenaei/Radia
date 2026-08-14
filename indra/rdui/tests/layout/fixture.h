/**
 * @file fixture.h
 * @brief Shared Layout Resource compiler test fixture.
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

#ifndef RD_TESTS_LAYOUT_FIXTURE_H
#define RD_TESTS_LAYOUT_FIXTURE_H

#include <map>
#include <memory>
#include <string>
#include <utility>
#include "diagnostic.h"
#include "layout/document.h"
#include "layout/resourcecompiler.h"

namespace tut {
struct LayoutCompilerFixture {
    std::map<std::string, std::string> resources;

    rdui::LayoutDocumentMap parseDocuments(rdui::DiagnosticResult& diagnostics) const {
        rdui::LayoutDocumentMap result;
        for (const auto& [resourceId, sourceXml] : resources) {
            rdui::LayoutDocumentParseResult parsed = rdui::LayoutDocumentParser().parse(sourceXml, resourceId);
            diagnostics.warnings.insert(diagnostics.warnings.end(), parsed.warnings.begin(), parsed.warnings.end());
            diagnostics.errors.insert(diagnostics.errors.end(), parsed.errors.begin(), parsed.errors.end());
            if (parsed.document) result.emplace(resourceId, std::shared_ptr<const rdui::LayoutDocument>(std::move(parsed.document)));
        }
        return result;
    }

    rdui::LayoutBuildResult buildWidgetTreeFromResource(const std::string& resourceId) const {
        rdui::LayoutBuildResult result;
        rdui::LayoutDocumentMap parsed = parseDocuments(result);
        if (result.hasErrors()) return result;
        return rdui::LayoutResourceCompiler(&parsed).buildWidgetTreeFromResource(resourceId);
    }

    rdui::LayoutBuildResult buildWidgetTreeFromString(const std::string& xml, const std::string& sourceName = {}) const {
        rdui::LayoutBuildResult result;
        rdui::LayoutDocumentMap parsed = parseDocuments(result);
        if (result.hasErrors()) return result;
        return rdui::LayoutResourceCompiler(&parsed).buildWidgetTreeFromString(xml, sourceName);
    }

    rdui::DiagnosticResult validateWidgetDefaults(const std::string& element) const {
        rdui::DiagnosticResult result;
        rdui::LayoutDocumentMap parsed = parseDocuments(result);
        if (result.hasErrors()) return result;
        return rdui::LayoutResourceCompiler(&parsed).validateWidgetDefaults(element);
    }
};
} // namespace tut

#endif // RD_TESTS_LAYOUT_FIXTURE_H
