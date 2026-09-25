/**
 * @file lldiriterator_test.cpp
 * @brief LLDirIterator test cases.
 *
 * $LicenseInfo:firstyear=2011&license=viewerlgpl$
 * Second Life Viewer Source Code
 * Copyright (C) 2011, Linden Research, Inc.
 *
 * This library is free software; you can redistribute it and/or
 * modify it under the terms of the GNU Lesser General Public
 * License as published by the Free Software Foundation;
 * version 2.1 of the License only.,
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
 * Linden Research, Inc., 945 Battery Street, San Francisco, CA  94111  USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include <filesystem>
#include <fstream>
#include <gtest/gtest.h>
#include "../../test/namedtempfile.h"
#include "../lldiriterator.h"

TEST(LLDirIterator, AcceptsSpecialCharactersInMask) {
    // CHOP-662 covered group names containing regular-expression characters.
    const std::filesystem::path directory = NamedTempFile::temp_path("radia-dir-iterator-");
    std::error_code error;
    ASSERT_TRUE(std::filesystem::create_directories(directory, error))
        << "Could not create test directory '"
        << directory.string()
        << "': "
        << error.message();

    struct Cleanup {
        std::filesystem::path path;
        ~Cleanup() {
            std::error_code cleanupError;
            std::filesystem::remove_all(path, cleanupError);
        }
    } cleanup{directory};

    struct MaskCase {
        const char* mask;
        const char* filename;
    };
    const MaskCase cases[] = {
        {"+bad-group-name]+?\?-??.*", "+bad-group-name]+ab-cd.txt"},
        {"))--@---bad-group-name2((?\?-??*.txt", "))--@---bad-group-name2((ab-cd.txt"},
        {"__^v--x)Cuide d sua vida(x--v^__?\?-??.*", "__^v--x)Cuide d sua vida(x--v^__ab-cd.log"},
    };

    for (const MaskCase& test : cases) {
        {
            std::ofstream output(directory / test.filename, std::ios::binary);
            ASSERT_TRUE(output.good()) << "Could not create test file '" << (directory / test.filename).string() << "'";
            output << "match";
            ASSERT_TRUE(output.good());
        }

        LLDirIterator iterator(directory, test.mask);
        std::string result;
        ASSERT_TRUE(iterator.next(result)) << "Mask did not match its special-character filename: " << test.mask;
        EXPECT_EQ(result, test.filename);
        EXPECT_FALSE(iterator.next(result));

        std::filesystem::remove(directory / test.filename, error);
        ASSERT_FALSE(error) << "Could not remove test file '" << (directory / test.filename).string() << "': " << error.message();
    }
}
