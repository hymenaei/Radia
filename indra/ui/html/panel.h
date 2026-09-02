/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "html/element.h"

namespace radia::ui {
class HTMLPanelElement : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

private:
    HTMLPanelElement();
};
} // namespace radia::ui
