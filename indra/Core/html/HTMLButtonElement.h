/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <string_view>
#include "HTMLElement.h"

namespace Core {
class HTMLButtonElement : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

public:
    bool defaultPointerEvents() const override { return true; }
    bool focusable() const override { return true; }
    AccessibleSemantics accessibleSemantics() const override;
    void paint(PaintContext& context, const Style::ComputedStyle& style, float scale) const override;

protected:
    explicit HTMLButtonElement(std::string_view elementName);
    void constrainResolvedStyle(Style::ComputedStyle& style) const override;

private:
    HTMLButtonElement();
};
} // namespace Core
