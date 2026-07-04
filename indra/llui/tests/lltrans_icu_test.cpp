/**
* @file lltrans_icu_test.cpp
* @brief Unit tests for LLTrans ICU MessageFormat
*
* $LicenseInfo:firstyear=2026&license=viewerlgpl$
* Lumen Viewer Source Code
* Copyright (C) 2026, Lumen Viewer Project.
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
*
* $/LicenseInfo$
*/

#include "linden_common.h"
#include "lltut.h"

namespace tut
{
    struct lltrans_icu_data
    {

    };
    typedef test_group<lltrans_icu_data> factory;
    typedef factory::object object;
}

namespace
{
    tut::factory tf("lltrans_icu");
}

namespace tut
{
    template<> template<>
    void object::test<1>()
    {
        ensure("P0 stub compiles", true);
    }
}
