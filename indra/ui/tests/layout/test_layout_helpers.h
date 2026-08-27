/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <map>
#include <string>
#include <utility>
#include "diagnostic.h"
#include "layout/resourcecompiler.h"
#include "resourceprovider.h"

namespace radia::ui::test {
using radia::ui::DiagnosticResult;
using radia::ui::LayoutBuildResult;
using radia::ui::LayoutResourceCompiler;
using radia::ui::ResourceSnapshot;

struct LayoutCompilerTestHelper {
    std::map<std::string, std::string> resources;

    ResourceSnapshot snapshot() const { return ResourceSnapshot(resources); }

    LayoutBuildResult buildElementTreeFromResource(const std::string& resourceId) const {
        ResourceSnapshot resourcesSnapshot = snapshot();
        return LayoutResourceCompiler(&resourcesSnapshot).buildElementTreeFromResource(resourceId);
    }

    LayoutBuildResult buildElementTreeFromString(const std::string& xml, const std::string& sourceName = {}) const {
        ResourceSnapshot resourcesSnapshot = snapshot();
        return LayoutResourceCompiler(&resourcesSnapshot).buildElementTreeFromString(xml, sourceName);
    }

    DiagnosticResult validateElementDefaults(const std::string& element) const {
        ResourceSnapshot resourcesSnapshot = snapshot();
        return LayoutResourceCompiler(&resourcesSnapshot).validateElementDefaults(element);
    }
};
} // namespace radia::ui::test
