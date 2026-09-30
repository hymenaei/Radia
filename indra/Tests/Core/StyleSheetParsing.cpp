/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/Color.h>
#include <Core/ComputedStyle.h>
#include <Core/StyleSheet.h>
#include <cstddef>
#include <gtest/gtest.h>
#include <string>

namespace {
using Core::Color;
using Core::CSS::StyleSheet;
using Core::Style::ComputedStyle;
} // namespace

TEST(StyleSheet, PreservesRulesAcrossCSSTokenBoundaries) {
    const std::string source = R"CSS(panel { width: 11px; content: "}"; background-image: url(icon\)name.svg);)CSS"
                               R"CSS( background-color: rgb(1, 2, 3); } /* } ; */ label { height: 13px; })CSS";
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(source, "tokens.css");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.errors.empty());
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 11.f);
    ASSERT_TRUE(stylesheet.resolve("panel", "", {}).content.has_value());
    EXPECT_EQ(*stylesheet.resolve("panel", "", {}).content, "}");
    ASSERT_EQ(stylesheet.resolve("panel", "", {}).backgroundLayers.size(), std::size_t(1));
    ASSERT_NE(stylesheet.resolve("panel", "", {}).backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*stylesheet.resolve("panel", "", {}).backgroundLayers.front().image.resource(), "icon)name.svg");
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheet, DiscardsClosedCommentsWithoutChangingSelectors) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("button/**/.primary { width: 13px; }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    EXPECT_EQ(stylesheet.resolve("button", "", {"primary"}).width().pixels(), 13.f);
}

TEST(StyleSheet, ReportsUnclosedComments) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { width: 13px; } /* unclosed", "comment.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_comment");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheet, KeepsValidDeclarationsAroundInvalidDeclaration) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel {\n  width: 11px;\n  malformed;\n  height: 13px;\n}", "declarations.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
    EXPECT_EQ(result.warnings.front().line, std::size_t(3));
    EXPECT_EQ(result.warnings.front().column, std::size_t(3));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 11.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheet, KeepsQualifiedRulePreludeTogether) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("bogus; label { height: 13px; }", "qualified.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t {1});
    EXPECT_EQ(result.warnings.front().code, "stylesheet.selector.element_unknown");
    EXPECT_TRUE(stylesheet.resolve("label", "", {}).height().isAuto());
}

TEST(StyleSheet, ReportsMalformedRootDeclarationOnce) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(":root { malformed; color: #ffffff; }", "root.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.declaration.invalid");
}

TEST(StyleSheet, KeepsNeighboringRulesAroundInvalidCSS) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { width: 11px; } @media screen { panel { width: 99px; } } "
                                             "button::unknown { width: 99px; } label { height: 13px; }",
        "rules.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(2));
    EXPECT_EQ(result.warnings[0].code, "stylesheet.at_rule.unsupported");
    EXPECT_EQ(result.warnings[1].code, "stylesheet.selector.pseudo_element_unknown");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 11.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheet, KeepsEarlierRangeValuesBeforeAnUnclosedRule) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { min-size: 11px 13px; }\nlabel { height: 13px;", "unclosed.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_block");
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_TRUE(style.minHeight().has_value());
    ASSERT_TRUE(style.minWidth().has_value());
    EXPECT_EQ(style.minHeight()->pixels(), 11.f);
    EXPECT_EQ(style.minWidth()->pixels(), 13.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheet, ParsesColorFunctionAtEOF) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { color: rgb(255 0 0 ", "unclosed-function.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), 1U);
    EXPECT_EQ(result.warnings.front().code, "stylesheet.syntax.unclosed_block");
    const Color color = stylesheet.resolve("panel", "", {}).color().resolvedColor();
    EXPECT_FLOAT_EQ(color.r, 1.f);
    EXPECT_FLOAT_EQ(color.g, 0.f);
    EXPECT_FLOAT_EQ(color.b, 0.f);
}
