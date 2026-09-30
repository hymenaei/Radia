/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include <Core/SystemFontProvider.h>
#include <gtest/gtest.h>

namespace TestCore {
using Core::detail::matchLocalFont;
using Core::detail::matchSystemFont;
using Core::Style::FontFamily;
using Core::Style::FontSelectionRequest;
using Core::Style::GenericFontFamily;

namespace {
const FontSelectionRequest kNormal {};
}

TEST(FontProvider, Generic) {
    const auto match = matchSystemFont(FontFamily {GenericFontFamily::SansSerif}, kNormal);

    ASSERT_TRUE(match);
    EXPECT_FALSE(match->path.empty());
    EXPECT_GE(match->faceIndex, 0);
    EXPECT_FALSE(match->familyName.empty());
}

TEST(FontProvider, SystemUI) {
    const auto match = matchSystemFont(FontFamily {GenericFontFamily::SystemUI}, kNormal);

    ASSERT_TRUE(match);
    EXPECT_FALSE(match->path.empty());
    EXPECT_GE(match->faceIndex, 0);
}

TEST(FontProvider, Family) {
    const auto generic = matchSystemFont(FontFamily {GenericFontFamily::SansSerif}, kNormal);
    ASSERT_TRUE(generic);

    const auto family = matchSystemFont(FontFamily {generic->familyName}, kNormal);

    ASSERT_TRUE(family);
    EXPECT_FALSE(family->path.empty());
    EXPECT_GE(family->faceIndex, 0);
}

TEST(FontProvider, Local) {
    const auto generic = matchSystemFont(FontFamily {GenericFontFamily::SansSerif}, kNormal);
    ASSERT_TRUE(generic);
    ASSERT_FALSE(generic->postScriptName.empty());

    const auto local = matchLocalFont(generic->postScriptName);

    ASSERT_TRUE(local);
    EXPECT_EQ(local->path, generic->path);
    EXPECT_EQ(local->faceIndex, generic->faceIndex);
}

TEST(FontProvider, Missing) {
    EXPECT_FALSE(matchSystemFont(FontFamily {std::string("RadiaMissingSystemFontFamily")}, kNormal));
    EXPECT_FALSE(matchLocalFont("RadiaMissingLocalFontFace"));
}
} // namespace TestCore
