/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "elements/element.h"

namespace radia::ui {
class IconElement : public Element {
    friend class detail::ElementDefinitionFactory;

public:
    explicit IconElement(std::string name = {});
    IconElement& setName(std::string name);
    const std::string& name() const { return mName; }
    void paint(PaintContext& context, const Style& style, float scale) const override;

private:
    std::string mName;
};
} // namespace radia::ui
