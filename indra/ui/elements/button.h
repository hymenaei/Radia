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
    void paint(PaintContext& context, const Style& style, float scale) const override;

protected:
    explicit ButtonElement(const char* elementName);
    void constrainResolvedStyle(Style& style) const override;
};
} // namespace radia::ui
