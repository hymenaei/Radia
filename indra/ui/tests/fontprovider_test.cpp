/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include <gtest/gtest.h>
#include "platform/fonts/SystemFontProvider.h"

namespace radia::ui::detail {
namespace { const FontSelectionRequest kNormal{}; }

TEST(FontProviderTest, Generic) {
    const auto match = matchSystemFont(FontFamily{GenericFontFamily::SansSerif}, kNormal);

    ASSERT_TRUE(match);
    EXPECT_FALSE(match->path.empty());
    EXPECT_GE(match->faceIndex, 0);
    EXPECT_FALSE(match->familyName.empty());
}

TEST(FontProviderTest, SystemUI) {
    const auto match = matchSystemFont(FontFamily{GenericFontFamily::SystemUI}, kNormal);

    ASSERT_TRUE(match);
    EXPECT_FALSE(match->path.empty());
    EXPECT_GE(match->faceIndex, 0);
}

TEST(FontProviderTest, Family) {
    const auto generic = matchSystemFont(FontFamily{GenericFontFamily::SansSerif}, kNormal);
    ASSERT_TRUE(generic);

    const auto family = matchSystemFont(FontFamily{generic->familyName}, kNormal);

    ASSERT_TRUE(family);
    EXPECT_FALSE(family->path.empty());
    EXPECT_GE(family->faceIndex, 0);
}

TEST(FontProviderTest, Local) {
    const auto generic = matchSystemFont(FontFamily{GenericFontFamily::SansSerif}, kNormal);
    ASSERT_TRUE(generic);
    ASSERT_FALSE(generic->postScriptName.empty());

    const auto local = matchLocalFont(generic->postScriptName);

    ASSERT_TRUE(local);
    EXPECT_EQ(local->path, generic->path);
    EXPECT_EQ(local->faceIndex, generic->faceIndex);
}

TEST(FontProviderTest, Missing) {
    EXPECT_FALSE(matchSystemFont(FontFamily{std::string("RadiaMissingSystemFontFamily")}, kNormal));
    EXPECT_FALSE(matchLocalFont("RadiaMissingLocalFontFace"));
}
} // namespace radia::ui::detail
