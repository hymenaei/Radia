/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstddef>
#include <gtest/gtest.h>
#include <utility>
#include "diagnostic.h"

namespace { using radia::ui::DiagnosticResult; }

TEST(DiagnosticResultTest, AppendsDiagnosticsPreservingSeverityAndOrder) {
    DiagnosticResult destination;
    destination.warning("warning.first", "first warning");
    destination.error("error.first", "first error");

    DiagnosticResult source;
    source.warning("warning.second", "second warning");
    source.warning("warning.third", "third warning");
    source.error("error.second", "second error");

    destination.append(std::move(source));

    EXPECT_EQ(destination.warnings.size(), std::size_t(3));
    EXPECT_EQ(destination.warnings[0].code, "warning.first");
    EXPECT_EQ(destination.warnings[1].code, "warning.second");
    EXPECT_EQ(destination.warnings[2].code, "warning.third");
    EXPECT_EQ(destination.errors.size(), std::size_t(2));
    EXPECT_EQ(destination.errors[0].code, "error.first");
    EXPECT_EQ(destination.errors[1].code, "error.second");
}
