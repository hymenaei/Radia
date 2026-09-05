/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include "dom/elementinternal.h"
#include "html/elementfactory.h"
#include "html/floater.h"
#include "resource/elementdefinition.h"

namespace radia::ui::test {
using detail::HTMLElementFactory;
using detail::makeElement;

inline void appendFloaterStructure(HTMLFloaterElement& floater, bool withClose = false, bool withMinimize = false) {
    auto head = makeElement<Element>("head");
    auto title = makeElement<Element>("title");
    title->textContent("title");
    head->append(std::move(title));
    if (withMinimize) head->append(HTMLElementFactory::Create("minimize"));
    if (withClose) head->append(HTMLElementFactory::Create("close"));
    floater.append(std::move(head));
    floater.append(makeElement<Element>("body"));
}

inline std::unique_ptr<HTMLFloaterElement> makeFloater(bool withClose = false, bool withMinimize = false) {
    auto floater = makeElement<HTMLFloaterElement>();
    appendFloaterStructure(*floater, withClose, withMinimize);
    return floater;
}
} // namespace radia::ui::test
