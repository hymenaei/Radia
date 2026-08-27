/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "componentmanager.h"

namespace radia::ui {
class Document;
class FloaterElement;
class Surface;
} // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::Surface;

class FloaterHost final : public ComponentManager::Host {
public:
    explicit FloaterHost(Surface& surface);

    void mount(Document& document) override;
    bool unmount(FloaterElement& root) override;
    bool replaceAll(std::vector<ReplacementRequest> replacements) override;
    bool clearAll(std::vector<FloaterElement*> roots) override;
    void present(FloaterElement& root) override;

private:
    Surface& mSurface;
};
} // namespace radia::viewer::ui
