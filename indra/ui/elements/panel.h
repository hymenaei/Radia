/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "elements/element.h"

namespace radia::ui {
class PanelElement : public Element {
    friend class detail::ElementDefinitionFactory;

public:
    PanelElement();
};
} // namespace radia::ui
