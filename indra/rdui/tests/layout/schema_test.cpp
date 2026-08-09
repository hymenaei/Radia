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
#include "../test/lltut.h"
#include "diagnostic.h"

namespace tut {
struct diagnostic_data {};
typedef test_group<diagnostic_data> diagnostic_test;
typedef diagnostic_test::object diagnostic_object;
using rduidiagnostic_object = diagnostic_object;
diagnostic_test diagnostic_testcase("diagnostic");

template<> template<> void rduidiagnostic_object::test<1>() {
    rdui::DiagnosticResult destination;
    destination.warning("warning.first", "first warning");
    destination.error("error.first", "first error");

    rdui::DiagnosticResult source;
    source.warning("warning.second", "second warning");
    source.warning("warning.third", "third warning");
    source.error("error.second", "second error");

    destination.append(std::move(source));

    ensure_equals("all warnings appended", destination.warnings.size(), std::size_t(3));
    ensure_equals("existing warning remains first", destination.warnings[0].code, std::string("warning.first"));
    ensure_equals("source warning order preserved", destination.warnings[1].code, std::string("warning.second"));
    ensure_equals("last source warning preserved", destination.warnings[2].code, std::string("warning.third"));
    ensure_equals("all errors appended", destination.errors.size(), std::size_t(2));
    ensure_equals("existing error remains first", destination.errors[0].code, std::string("error.first"));
    ensure_equals("source error follows", destination.errors[1].code, std::string("error.second"));
}

template<> template<> void rduidiagnostic_object::test<2>() {
    rdui::DiagnosticResult destination;
    rdui::DiagnosticResult empty;

    destination.append(std::move(empty));

    ensure("empty append has no errors", !destination.hasErrors());
    ensure("empty append has no warnings", destination.warnings.empty());
}
} // namespace tut
