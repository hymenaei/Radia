/**
 * @file alfontshaping.h
 * @brief FriBidi paragraph layout and HarfBuzz text shaping.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Alchemy Viewer Source Code
 * Copyright (C) 2026, Rye <rye@alchemyviewer.org>
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

#pragma once

#include <utility>
#include <vector>

#include "llstring.h"

class LLFontFreetype;

struct ALShapedGlyph
{
    const LLFontFreetype* face;
    U32                   glyph_id;
    S32                   cluster;
    F32                   x_advance;
    F32                   y_advance;
    F32                   x_offset;
    F32                   y_offset;
};

struct ALFontShapeLayout
{
    std::vector<std::pair<size_t, size_t>>         ranges;
    std::vector<const std::vector<ALShapedGlyph>*> glyphs;
    size_t                                         mutation_snapshot = 0;

    bool valid() const;
};

namespace ALFontShaping
{
    // Shape wstr[begin..end) using root_face's fallback chain and return the
    // glyph stream in logical/visual paint order. Clusters in the cached
    // result are slice-local; shapeRun rebases them to the caller's string.
    void shapeRun(const LLFontFreetype* root_face,
                  LLWStringView         wstr,
                  size_t                begin,
                  size_t                end,
                  std::vector<ALShapedGlyph>& out_glyphs);

    // Return a cached glyph stream for wstr[begin..end). The reference is
    // invalidated by a cache mutation, so callers must consume it promptly.
    const std::vector<ALShapedGlyph>& shapeLine(
        const LLFontFreetype* root_face,
        LLWStringView         wstr,
        size_t                begin,
        size_t                end,
        bool                  disable_optional_ligatures = false);

    // Build the current line layout. The layout records a cache mutation
    // snapshot so holders can assert that its glyph references remain valid.
    ALFontShapeLayout layoutLine(
        const LLFontFreetype* root_face,
        LLWStringView         slice,
        bool                  disable_optional_ligatures = false);

    void clearCache();

    void clearCacheForFace(const LLFontFreetype* face);

    size_t cacheSize();

    size_t cacheMutationCount();
}
