/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "HTMLElement.h"

namespace Core {
class HTMLPanelElement : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

private:
    HTMLPanelElement();
};
} // namespace Core
