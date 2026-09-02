/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstdint>
#include <memory>
#include "dom/elementinternal.h"
#include "surface/surface.h"

namespace radia::ui {
struct Surface::ElementSnapshot {
    ElementRef<Element> lifetime;
    const Element* element = nullptr;
    const Surface* surface = nullptr;
    const Element* parent = nullptr;
    std::uint64_t layoutRevision = 0;
    std::uint64_t styleRevision = 0;
    std::shared_ptr<char> mountLifetime;
};
} // namespace radia::ui
