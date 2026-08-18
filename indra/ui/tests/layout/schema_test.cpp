/**
 * @file schema_test.cpp
 * @brief Tests Layout Resource schema validation and diagnostic reporting.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.
 *
 * This library is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the GNU
 * Lesser General Public License for more details.
 *
 * You should have received a copy of the GNU Lesser General Public
 * License along with this library; if not, write to the Free Software
 * Foundation, Inc., 51 Franklin Street, Fifth Floor, Boston, MA  02110-1301  USA
 * $/LicenseInfo$
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

TEST(DiagnosticResultTest, AppendingEmptyResultLeavesDiagnosticsUnchanged) {
    DiagnosticResult destination;
    DiagnosticResult empty;

    destination.append(std::move(empty));

    EXPECT_FALSE(destination.hasErrors());
    EXPECT_TRUE(destination.warnings.empty());
}
