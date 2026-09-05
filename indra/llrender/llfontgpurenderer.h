/**
 * @file llfontgpurenderer.h
 * @brief Coordinates shaped text, glyph caching, and GPU batches for analytic font rendering.
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

#ifndef LL_LLFONTGPURENDERER_H
#define LL_LLFONTGPURENDERER_H

#include "llfontgl.h"
#include "alfontshaping.h"
#include "llhbgpu.h"
#if LL_HAS_HB_GPU

class LLColor4U;
class LLFontFreetype;

class LLFontGpuRenderer final {
public:
    struct Request {
        const LLFontFreetype& font;
        const LLWString& text;
        const ALFontShapeLayout& layout;
        const LLColor4& color;
        const LLColor4U& shadowColor;
        LLFontGL::pass_boundary_cb_t passBoundary;
        S32 beginOffset;
        S32 textLength;
        F32 penX;
        F32 penY;
        F32 startX;
        S32 scaledMaxPixels;
        F32 italicSlant;
        U8 style;
        LLFontGL::ShadowType shadow;
        bool useSubpixelPen;
        bool useColor;
    };

    struct Result {
        bool rendered = false;
        S32 charsDrawn = 0;
        F32 penX = 0.f;
        bool emittedFixedColorGlyph = false;
    };

    static Result tryRender(const Request& request);
    static void destroyGL();
};
#endif // LL_HAS_HB_GPU
#endif // LL_LLFONTGPURENDERER_H
