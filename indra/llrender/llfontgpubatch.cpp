/**
 * @file llfontgpubatch.cpp
 * @brief Glyph-quad geometry helper for the analytic font path.
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

#include "linden_common.h"
#include "llfontgpubatch.h"
#if LL_HAS_HB_GPU
#include "llvertexbuffer.h"

namespace LLFontGpuBatch {
void buildGlyphQuad(LLVector4a* positions, LLVector2* texcoords, LLColor4U* colors, U32* glyphLocations,
                    const LLFontGpuGlyphCache::GlyphLoc& location, F32 penX, F32 penY, F32 scale, F32 slant, const LLColor4U& color,
                    U32 glyphLocation) {
    const F32 lu = (F32)location.mXBearing;
    const F32 ru = (F32)(location.mXBearing + location.mWidth);
    const F32 tu = (F32)location.mYBearing;
    const F32 bu = (F32)(location.mYBearing + location.mHeight);

    constexpr F32 D = 0.5f;
    const F32 dEm = (scale != 0.f) ? (D / scale) : 0.f;

    struct Corner {
        F32 u, v, nx, ny, shear;
    };
    const Corner corners[6] = {
        {lu, tu, -1.f, 1.f, 0.f},    // TL
        {lu, bu, -1.f, -1.f, slant}, // BL
        {ru, bu, 1.f, -1.f, slant},  // BR
        {lu, tu, -1.f, 1.f, 0.f},    // TL
        {ru, bu, 1.f, -1.f, slant},  // BR
        {ru, tu, 1.f, 1.f, 0.f},     // TR
    };

    for (S32 i = 0; i < 6; ++i) {
        const Corner& c = corners[i];
        positions[i].set(penX + c.u * scale + c.shear + c.nx * D, penY + c.v * scale + c.ny * D, 0.f);
        texcoords[i].set(c.u + c.nx * dEm, c.v + c.ny * dEm);
        colors[i] = color;
        glyphLocations[i] = glyphLocation;
    }
}
} // namespace LLFontGpuBatch
#endif // LL_HAS_HB_GPU
