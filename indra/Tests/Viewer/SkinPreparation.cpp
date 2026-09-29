/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Viewer/SkinPreparation.h>
#include <gtest/gtest.h>
#include <utility>

namespace {
using Viewer::prepareSkinGeneration;
using Viewer::SkinSnapshotResult;

SkinSnapshotResult validSnapshot() {
    SkinSnapshotResult result;
    result.snapshot.add("localization.yaml", "defaultLocale: en\nlocales: {en: {strings: {}}}\n");
    result.snapshot.add("skin.css", "");
    return result;
}
} // namespace

TEST(SkinPreparation, CompilesSnapshot) {
    const auto result = prepareSkinGeneration(validSnapshot());

    ASSERT_TRUE(result.ok());
    ASSERT_NE(result.generation, nullptr);
}

TEST(SkinPreparation, PreservesDiagnosticsOnError) {
    SkinSnapshotResult captured = validSnapshot();
    captured.error("skin.test.rejected", "The test capture was rejected.");

    const auto result = prepareSkinGeneration(std::move(captured));

    ASSERT_FALSE(result.ok());
    ASSERT_EQ(result.errors.size(), 1U);
    EXPECT_EQ(result.errors.front().code, "skin.test.rejected");
    EXPECT_EQ(result.generation, nullptr);
}
