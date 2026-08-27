/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "elements/element.h"

namespace radia::ui {
class LabelElement : public Element {
    friend class detail::ElementDefinitionFactory;
    friend class detail::ElementCompilerAccess;

public:
    explicit LabelElement(std::string text = {});

    bool defaultPointerEvents() const override { return static_cast<bool>(mTarget); }

protected:
    LabelElement& setTargetId(std::string id);

private:
    void onActivate() override;

    std::string mTargetId;
    Element* mTarget = nullptr;
};
} // namespace radia::ui
