/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <vector>
#include "ComponentManager.h"

namespace Core {
class Document;
class HTMLFloaterElement;
class Surface;
} // namespace Core

namespace Viewer {
class FloaterHost final : public ComponentManager::Host {
public:
    explicit FloaterHost(Core::Surface& surface);

    bool mount(Core::Document& document) override;
    bool unmount(Core::HTMLFloaterElement& root) override;
    bool replaceAll(std::vector<ReplacementRequest> replacements) override;
    bool clearAll(std::vector<Core::HTMLFloaterElement*> roots) override;
    void present(Core::HTMLFloaterElement& root) override;

private:
    Core::Surface& mSurface;
};
} // namespace Viewer
