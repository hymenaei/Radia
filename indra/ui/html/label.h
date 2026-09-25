/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "html/element.h"

namespace radia::ui {
class HTMLLabelElement : public HTMLElement {
    friend class dom_detail::FragmentParser;
    friend class detail::ElementDefinitions;
    friend class detail::ElementConstructionAccess;
    friend class detail::HTMLElementFactory;

public:
    const std::string& targetId() const { return mTargetId; }
    bool defaultPointerEvents() const override { return target() != nullptr; }
    const Element* target() const;
    AccessibleSemantics accessibleSemantics() const override;

protected:
    HTMLLabelElement& setTargetId(std::string id);

private:
    explicit HTMLLabelElement(std::string text = {});
    Element* target();

    void onActivate() override;

    std::string mTargetId;
};
} // namespace radia::ui
