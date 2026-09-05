/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "html/element.h"

namespace radia::ui {
class HTMLIconElement : public HTMLElement {
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

public:
    HTMLIconElement& setName(std::string name);
    const std::string& name() const { return mName; }
    void paint(PaintContext& context, const ComputedStyle& style, float scale) const override;

protected:
private:
    explicit HTMLIconElement(std::string name = {});

    std::string mName;
};
} // namespace radia::ui
