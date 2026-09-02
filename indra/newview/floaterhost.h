/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "componentmanager.h"

namespace radia::ui {
class Document;
class HTMLFloaterElement;
class Surface;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::Surface;

class FloaterHost final : public ComponentManager::Host {
public:
    explicit FloaterHost(Surface& surface);

    void mount(Document& document) override;
    bool unmount(HTMLFloaterElement& root) override;
    bool replaceAll(std::vector<ReplacementRequest> replacements) override;
    bool clearAll(std::vector<HTMLFloaterElement*> roots) override;
    void present(HTMLFloaterElement& root) override;

private:
    Surface& mSurface;
};
} // namespace radia::viewer::ui
