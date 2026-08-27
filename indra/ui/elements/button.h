/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "elements/element.h"

namespace radia::ui {
class ButtonElement : public Element {
    friend class detail::ElementDefinitionFactory;

public:
    ButtonElement();
    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }

protected:
    explicit ButtonElement(const char* elementName);
};
} // namespace radia::ui
