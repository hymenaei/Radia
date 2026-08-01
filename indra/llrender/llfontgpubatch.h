/**
 * @file llfontgpubatch.h
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

#ifndef LL_LLFONTGPUBATCH_H
#define LL_LLFONTGPUBATCH_H

#include "llhbgpu.h"
#include "stdtypes.h"
#if LL_HAS_HB_GPU
    #include "llfontgpuglyphcache.h"

class LLVector4a;
class LLVector2;
class LLColor4U;

namespace LLFontGpuBatch {
void buildGlyphQuad(LLVector4a* pos, LLVector2* uv, LLColor4U* col, U32* gloc, const LLFontGpuGlyphCache::GlyphLoc& loc, F32 pen_x, F32 pen_y,
                    F32 scale, F32 slant, const LLColor4U& color, U32 glyph_loc);
} // namespace LLFontGpuBatch
#endif // LL_HAS_HB_GPU
#endif // LL_LLFONTGPUBATCH_H
