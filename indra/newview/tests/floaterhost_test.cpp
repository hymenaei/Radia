/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <memory>
#include "elements/document.h"
#include "elements/elementinternal.h"
#include "elements/floater.h"
#include "floaterhost.h"
#include "style/stylesheet.h"
#include "surface/surface.h"

namespace {
using radia::ui::Document;
using radia::ui::FloaterElement;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::viewer::ui::FloaterHost;

constexpr char kFloaterStyles[] = "floater { size: 100px 80px; min-size: 40px 30px; display: flex; flex-direction: column; } "
                                  "floater > head { height: 20px; } floater > body { flex-grow: 1; }";

void appendFloaterStructure(FloaterElement& floater) {
    auto head = std::make_unique<radia::ui::Element>("head");
    auto title = std::make_unique<radia::ui::Element>("title");
    title->textContent("title");
    head->append(std::move(title));
    floater.append(std::move(head));
    floater.append(std::make_unique<radia::ui::Element>("body"));
}
} // namespace

TEST(FloaterHostTest, ReplacesMountedFloaterThroughSurfaceSeam) {
    auto currentDocument = std::make_unique<Document>(std::make_unique<FloaterElement>());
    FloaterElement* current = dynamic_cast<FloaterElement*>(currentDocument->documentElement());
    ASSERT_NE(current, nullptr);
    appendFloaterStructure(*current);
    current->setResizeable(true);

    auto replacementDocument = std::make_unique<Document>(std::make_unique<FloaterElement>());
    FloaterElement* replacement = dynamic_cast<FloaterElement*>(replacementDocument->documentElement());
    ASSERT_NE(replacement, nullptr);
    appendFloaterStructure(*replacement);
    replacement->setResizeable(true);

    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterStyles).ok());
    Surface surface(styleSheet);
    surface.setViewport(300.f, 200.f);
    FloaterHost host(surface);

    host.mount(*currentDocument);
    ASSERT_TRUE(surface.ownsFloater(*current));
    ASSERT_TRUE(surface.prepareFloater(*current).has_value());

    const Rect userRect{30.f, 40.f, 140.f, 100.f};
    surface.placeFloater(*current, userRect);
    surface.updateLayout();

    ASSERT_TRUE(host.replaceAll({{current, replacementDocument.get()}}));
    EXPECT_FALSE(surface.ownsFloater(*current));
    EXPECT_TRUE(surface.ownsFloater(*replacement));
    EXPECT_FLOAT_EQ(replacement->rect().x, userRect.x);
    EXPECT_FLOAT_EQ(replacement->rect().y, userRect.y);
    EXPECT_FLOAT_EQ(replacement->rect().w, userRect.w);
    EXPECT_FLOAT_EQ(replacement->rect().h, userRect.h);
}
