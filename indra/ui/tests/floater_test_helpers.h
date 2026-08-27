/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include "elements/elementinternal.h"
#include "elements/floater.h"

namespace radia::ui::test {
inline void appendFloaterStructure(FloaterElement& floater, bool withClose = false, bool withMinimize = false) {
    auto head = std::make_unique<Element>("head");
    auto title = std::make_unique<Element>("title");
    title->textContent("title");
    head->append(std::move(title));
    if (withMinimize) head->append(detail::createRuntimeElement("minimize"));
    if (withClose) head->append(detail::createRuntimeElement("close"));
    floater.append(std::move(head));
    floater.append(std::make_unique<Element>("body"));
}

inline std::unique_ptr<FloaterElement> makeFloater(bool withClose = false, bool withMinimize = false) {
    auto floater = std::make_unique<FloaterElement>();
    appendFloaterStructure(*floater, withClose, withMinimize);
    return floater;
}
} // namespace radia::ui::test
