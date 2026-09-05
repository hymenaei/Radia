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
using radia::ui::ResourceBuildResult;
using radia::ui::ResourceCompiler;
using radia::ui::ResourceSnapshot;

struct ResourceCompilerTestHelper {
    std::map<std::string, std::string> resources;

    ResourceSnapshot snapshot() const { return ResourceSnapshot(resources); }

    ResourceBuildResult buildElementTreeFromResource(const ResourceId& id) const {
        ResourceSnapshot resourcesSnapshot = snapshot();
        return ResourceCompiler(&resourcesSnapshot).buildElementTreeFromResource(id);
    }

    ResourceBuildResult buildElementTreeFromString(const std::string& html, const std::string& sourceName = {}) const {
        ResourceSnapshot resourcesSnapshot = snapshot();
        return ResourceCompiler(&resourcesSnapshot).buildElementTreeFromString(html, sourceName);
    }

    DiagnosticResult validateElementDefaults(const std::string& elementName) const {
        ResourceSnapshot resourcesSnapshot = snapshot();
        return ResourceCompiler(&resourcesSnapshot).validateElementDefaults(elementName);
    }
};
} // namespace radia::ui::test
