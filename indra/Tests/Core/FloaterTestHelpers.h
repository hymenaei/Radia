/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <Core/ElementInternal.h>
#include <Core/HTMLElementFactory.h>
#include <Core/HTMLFloaterElement.h>
#include <Core/ResourceElementDefinition.h>
#include <memory>

namespace CoreTests {
using Core::Element;
using Core::HTMLFloaterElement;
using Core::detail::HTMLElementFactory;
using Core::detail::makeElement;

inline void appendFloaterStructure(HTMLFloaterElement& floater, bool withClose = false, bool withMinimize = false) {
    auto head = makeElement<Element>("head");
    auto title = makeElement<Element>("title");
    title->textContent("title");
    head->append(std::move(title));
    if (withMinimize)
        head->append(HTMLElementFactory::create("minimize"));
    if (withClose)
        head->append(HTMLElementFactory::create("close"));
    floater.append(std::move(head));
    floater.append(makeElement<Element>("body"));
}

inline std::unique_ptr<HTMLFloaterElement> makeFloater(bool withClose = false, bool withMinimize = false) {
    auto floater = makeElement<HTMLFloaterElement>();
    appendFloaterStructure(*floater, withClose, withMinimize);
    return floater;
}
} // namespace CoreTests
