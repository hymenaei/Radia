/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <gtest/gtest.h>
#include <memory>
#include <utility>
#include <vector>
#include "css/stylesheet.h"
#include "dom/document.h"
#include "dom/elementinternal.h"
#include "floaterhost.h"
#include "html/floater.h"
#include "surface/surface.h"

namespace {
using radia::ui::Document;
using radia::ui::Element;
using radia::ui::HTMLFloaterElement;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Surface;
using radia::ui::detail::ElementInternalAccess;
using radia::ui::detail::makeElement;
using radia::viewer::ui::ComponentManager;
using radia::viewer::ui::FloaterHost;

constexpr char kFloaterStyles[] = "floater { size: 100px 80px; min-size: 40px 30px; display: flex; flex-direction: column; } "
                                  "floater > head { height: 20px; } floater > body { flex-grow: 1; }";

void appendFloaterStructure(HTMLFloaterElement& floater) {
    auto head = makeElement<Element>("head");
    auto title = makeElement<Element>("title");
    title->textContent("title");
    head->append(std::move(title));
    floater.append(std::move(head));
    floater.append(makeElement<Element>("body"));
}
} // namespace

TEST(FloaterHostTest, ReplacesMountedFloaterThroughSurfaceSeam) {
    auto currentDocument = std::make_unique<Document>(makeElement<HTMLFloaterElement>());
    HTMLFloaterElement* current = dynamic_cast<HTMLFloaterElement*>(currentDocument->documentElement());
    ASSERT_NE(current, nullptr);
    appendFloaterStructure(*current);
    current->setResizeable(true);

    auto replacementDocument = std::make_unique<Document>(makeElement<HTMLFloaterElement>());
    HTMLFloaterElement* replacement = dynamic_cast<HTMLFloaterElement*>(replacementDocument->documentElement());
    ASSERT_NE(replacement, nullptr);
    appendFloaterStructure(*replacement);
    replacement->setResizeable(true);

    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterStyles).ok());
    Surface surface(styleSheet);
    surface.setViewport(300.f, 200.f);
    FloaterHost host(surface);

    ASSERT_TRUE(host.mount(*currentDocument));
    ASSERT_TRUE(surface.ownsFloater(*current));
    ASSERT_TRUE(surface.prepareFloater(*current).has_value());

    const Rect userRect{30.f, 40.f, 140.f, 100.f};
    surface.placeFloater(*current, userRect);
    surface.updateLayout();

    ComponentManager::Host::ReplacementRequest request{current, replacementDocument.get()};
    std::vector<ComponentManager::Host::ReplacementRequest> requests;
    requests.push_back(std::move(request));
    ASSERT_TRUE(host.replaceAll(std::move(requests)));
    EXPECT_FALSE(surface.ownsFloater(*current));
    EXPECT_TRUE(surface.ownsFloater(*replacement));
    EXPECT_FLOAT_EQ(replacement->rect().x, userRect.x);
    EXPECT_FLOAT_EQ(replacement->rect().y, userRect.y);
    EXPECT_FLOAT_EQ(replacement->rect().w, userRect.w);
    EXPECT_FLOAT_EQ(replacement->rect().h, userRect.h);
}

TEST(FloaterHostTest, RejectsClearWhenAnyRootIsNotMounted) {
    auto currentDocument = std::make_unique<Document>(makeElement<HTMLFloaterElement>());
    HTMLFloaterElement* current = dynamic_cast<HTMLFloaterElement*>(currentDocument->documentElement());
    ASSERT_NE(current, nullptr);
    appendFloaterStructure(*current);

    auto unmountedDocument = std::make_unique<Document>(makeElement<HTMLFloaterElement>());
    HTMLFloaterElement* unmounted = dynamic_cast<HTMLFloaterElement*>(unmountedDocument->documentElement());
    ASSERT_NE(unmounted, nullptr);
    appendFloaterStructure(*unmounted);

    StyleSheet styleSheet;
    ASSERT_TRUE(styleSheet.loadRadia(kFloaterStyles).ok());
    Surface surface(styleSheet);
    surface.setViewport(300.f, 200.f);
    FloaterHost host(surface);

    ASSERT_TRUE(host.mount(*currentDocument));
    ASSERT_TRUE(surface.ownsFloater(*current));

    EXPECT_FALSE(host.clearAll({current, unmounted}));
    EXPECT_TRUE(surface.ownsFloater(*current));
    EXPECT_FALSE(current->closed());
}

TEST(FloaterHostTest, RejectsInvalidMountWithoutChangingTheSurface) {
    auto document = std::make_unique<Document>(makeElement<Element>("panel"));
    Element* root = document->documentElement();
    ASSERT_NE(root, nullptr);

    StyleSheet styleSheet;
    Surface surface(styleSheet);
    FloaterHost host(surface);

    EXPECT_FALSE(host.mount(*document));
    EXPECT_FALSE(ElementInternalAccess::isMounted(*root));
    EXPECT_FALSE(surface.hasVisibleFloater());
}
