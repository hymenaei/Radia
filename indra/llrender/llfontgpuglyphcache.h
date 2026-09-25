/**
 * @file llfontgpuglyphcache.h
 * @brief Atlas-free glyph store for the analytic font path.
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

#ifndef LL_LLFONTGPUGLYPHCACHE_H
#define LL_LLFONTGPUGLYPHCACHE_H

#include "llhbgpu.h"
#include "stdtypes.h"
#if LL_HAS_HB_GPU
    #include <vector>
    #include <boost/unordered/unordered_flat_map.hpp>

class LLFontGpuGlyphCache {
public:
    struct GlyphLoc {
        U32 mTexelOffset = 0;
        U32 mTexelCount = 0;
        S32 mXBearing = 0;
        S32 mYBearing = 0;
        S32 mWidth = 0;
        S32 mHeight = 0;
        bool drawable() const { return mTexelCount != 0; }
    };

    class Batch {
    public:
        Batch(Batch&& other) noexcept;
        ~Batch();

        Batch(const Batch&) = delete;
        Batch& operator=(const Batch&) = delete;
        Batch& operator=(Batch&&) = delete;

        S32 generation() const { return mGeneration; }

    private:
        friend class LLFontGpuGlyphCache;
        explicit Batch(S32 generation);

        S32 mGeneration = 0;
    };

    static constexpr U32 kBytesPerTexel = 8;

    LLFontGpuGlyphCache();
    ~LLFontGpuGlyphCache();

    LLFontGpuGlyphCache(const LLFontGpuGlyphCache&) = delete;
    LLFontGpuGlyphCache& operator=(const LLFontGpuGlyphCache&) = delete;

    void init(hb_font_t* sourceFont, bool color = false, unsigned palette = 0);

    bool isColor() const { return mColor; }

    static Batch beginBatch();

    const GlyphLoc& getOrEncodeGlyph(const Batch& batch, U32 glyphId);

    static bool bindBufferTexture(U32 texture_unit);
    static bool bindBufferTexture() { return bindBufferTexture(0); }

    static void destroyGL();

    #ifdef LL_TEST
    static void resetForTesting() { reset(); }
    #endif
    static S32 getGeneration() { return sGeneration; }
    U32 getGlyphCount() const { return static_cast<U32>(mCache.size()); }
    static U32 getArenaTexels() { return static_cast<U32>(sArena.size()) / kBytesPerTexel; }

    static void setMaxTexels(U32 maxTexels) { sMaxTexels = maxTexels; }

private:
    static void reset();

    void ensureEncoder();
    static bool ensureGLBuffer();
    GlyphLoc encodeGlyph(U32 glyphId);
    GlyphLoc encodeDrawGlyph(U32 glyphId);
    GlyphLoc encodePaintGlyph(U32 glyphId);

    static constexpr U32 kDefaultMaxTexels = 1u << 20;

    hb_gpu_draw_t* mEncoder = nullptr;
    hb_gpu_paint_t* mPaintEncoder = nullptr;
    hb_font_t* mEncodeFont = nullptr;
    bool mColor = false;
    unsigned mPalette = 0;

    static std::vector<U8> sArena;
    static U32 sUploadedBytes;
    static U32 sMaxTexels;
    static U32 sGLBuffer;
    static U32 sGLTexture;
    static U32 sGLCapacityBytes;
    static S32 sGeneration;
    static bool sResetPending;
    static U32 sActiveBatches;

    boost::unordered_flat_map<U32, GlyphLoc> mCache;
    S32 mLastArenaGen = 0;
};
#endif // LL_HAS_HB_GPU
#endif // LL_LLFONTGPUGLYPHCACHE_H
