/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/Diagnostic.h>
#include <Core/ResourceCompiler.h>
#include <Core/ResourceProvider.h>
#include <map>
#include <string>
#include <utility>

namespace CoreTests {
using Core::DiagnosticResult;
using Core::ResourceBuildResult;
using Core::ResourceCompiler;
using Core::ResourceId;
using Core::ResourceSnapshot;

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
} // namespace CoreTests
