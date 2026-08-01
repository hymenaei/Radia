/**
 * @file llfontgpuglyphcache.cpp
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

#include "linden_common.h"
#include "llfontgpuglyphcache.h"
#if LL_HAS_HB_GPU
    #include "llgl.h"
    #include "llglheaders.h"
    #include "llrender.h"

std::vector<U8> LLFontGpuGlyphCache::sArena;
U32 LLFontGpuGlyphCache::sUploadedBytes = 0;
U32 LLFontGpuGlyphCache::sMaxTexels = LLFontGpuGlyphCache::kDefaultMaxTexels;
U32 LLFontGpuGlyphCache::sGLBuffer = 0;
U32 LLFontGpuGlyphCache::sGLTexture = 0;
U32 LLFontGpuGlyphCache::sGLCapacityBytes = 0;
S32 LLFontGpuGlyphCache::sGeneration = 1;
bool LLFontGpuGlyphCache::sResetPending = false;
U32 LLFontGpuGlyphCache::sActiveBatches = 0;

LLFontGpuGlyphCache::Batch::Batch(S32 generation) : mGeneration(generation) {
    ++sActiveBatches;
}

LLFontGpuGlyphCache::Batch::Batch(Batch&& other) noexcept : mGeneration(other.mGeneration) {
    other.mGeneration = 0;
}

LLFontGpuGlyphCache::Batch::~Batch() {
    if (mGeneration) {
        llassert(sActiveBatches > 0);
        --sActiveBatches;
    }
}

LLFontGpuGlyphCache::LLFontGpuGlyphCache() : mLastArenaGen(sGeneration) {}

LLFontGpuGlyphCache::~LLFontGpuGlyphCache() {
    if (mEncoder) {
        hb_gpu_draw_destroy(mEncoder);
        mEncoder = nullptr;
    }
    if (mPaintEncoder) {
        hb_gpu_paint_destroy(mPaintEncoder);
        mPaintEncoder = nullptr;
    }
    if (mEncodeFont) {
        hb_font_destroy(mEncodeFont);
        mEncodeFont = nullptr;
    }
}

void LLFontGpuGlyphCache::init(hb_font_t* src_font, bool color, unsigned palette) {
    mCache.clear();
    mLastArenaGen = sGeneration;

    mColor = color;
    mPalette = palette;

    if (mEncodeFont) {
        hb_font_destroy(mEncodeFont);
        mEncodeFont = nullptr;
    }

    hb_face_t* face = src_font ? hb_font_get_face(src_font) : nullptr;
    if (face) {
        unsigned upem = hb_face_get_upem(face);
        mEncodeFont = hb_font_create(face);
        hb_font_set_scale(mEncodeFont, static_cast<int>(upem), static_cast<int>(upem));

        unsigned num_coords = 0;
        const int* coords = hb_font_get_var_coords_normalized(src_font, &num_coords);
        if (coords && num_coords) hb_font_set_var_coords_normalized(mEncodeFont, coords, num_coords);
    }
}

void LLFontGpuGlyphCache::ensureEncoder() {
    if (mColor) {
        if (!mPaintEncoder) {
            mPaintEncoder = hb_gpu_paint_create_or_fail();
            if (mPaintEncoder) hb_gpu_paint_set_palette(mPaintEncoder, mPalette);
        }
    } else if (!mEncoder) mEncoder = hb_gpu_draw_create_or_fail();
}

LLFontGpuGlyphCache::GlyphLoc LLFontGpuGlyphCache::encodeGlyph(U32 glyph_id) {
    GlyphLoc loc;
    if (!mEncodeFont) return loc;

    ensureEncoder();
    return mColor ? encodePaintGlyph(glyph_id) : encodeDrawGlyph(glyph_id);
}

namespace {
void store_blob(std::vector<U8>& arena, hb_blob_t* blob, const hb_glyph_extents_t& ext, LLFontGpuGlyphCache::GlyphLoc& loc) {
    unsigned len = blob ? hb_blob_get_length(blob) : 0;
    len -= len % LLFontGpuGlyphCache::kBytesPerTexel;
    if (blob && len >= LLFontGpuGlyphCache::kBytesPerTexel) {
        const char* data = hb_blob_get_data(blob, nullptr);
        U32 byte_off = static_cast<U32>(arena.size());
        arena.insert(arena.end(), data, data + len);

        loc.mTexelOffset = byte_off / LLFontGpuGlyphCache::kBytesPerTexel;
        loc.mTexelCount = len / LLFontGpuGlyphCache::kBytesPerTexel;
        loc.mXBearing = ext.x_bearing;
        loc.mYBearing = ext.y_bearing;
        loc.mWidth = ext.width;
        loc.mHeight = ext.height;
    }
}
} // namespace

LLFontGpuGlyphCache::GlyphLoc LLFontGpuGlyphCache::encodeDrawGlyph(U32 glyph_id) {
    GlyphLoc loc;
    if (!mEncoder) return loc;

    hb_gpu_draw_glyph(mEncoder, mEncodeFont, static_cast<hb_codepoint_t>(glyph_id));

    hb_glyph_extents_t ext = {0, 0, 0, 0};
    hb_blob_t* blob = hb_gpu_draw_encode(mEncoder, &ext);
    store_blob(sArena, blob, ext, loc);

    if (blob) hb_gpu_draw_recycle_blob(mEncoder, blob);
    return loc;
}

LLFontGpuGlyphCache::GlyphLoc LLFontGpuGlyphCache::encodePaintGlyph(U32 glyph_id) {
    GlyphLoc loc;
    if (!mPaintEncoder) return loc;

    hb_gpu_paint_glyph(mPaintEncoder, mEncodeFont, static_cast<hb_codepoint_t>(glyph_id));

    hb_glyph_extents_t ext = {0, 0, 0, 0};
    hb_blob_t* blob = hb_gpu_paint_encode(mPaintEncoder, &ext);
    store_blob(sArena, blob, ext, loc);

    if (blob) hb_gpu_paint_recycle_blob(mPaintEncoder, blob);
    return loc;
}

LLFontGpuGlyphCache::Batch LLFontGpuGlyphCache::beginBatch() {
    llassert(sActiveBatches == 0);
    if (sResetPending) {
        gGL.flush();
        reset();
    }
    return Batch(sGeneration);
}

const LLFontGpuGlyphCache::GlyphLoc& LLFontGpuGlyphCache::getGlyph(const Batch& batch, U32 glyph_id) {
    static const GlyphLoc sEmpty;
    llassert(batch.mGeneration != 0);
    llassert(batch.mGeneration == sGeneration);
    if (!mEncodeFont) return sEmpty;

    if (mLastArenaGen != sGeneration) {
        mCache.clear();
        mLastArenaGen = sGeneration;
    }

    if (auto it = mCache.find(glyph_id); it != mCache.end()) return it->second;

    const bool had_prior = !sArena.empty();
    GlyphLoc loc = encodeGlyph(glyph_id);

    if (loc.drawable() && getArenaTexels() > sMaxTexels && had_prior) sResetPending = true;

    auto [it, ok] = mCache.emplace(glyph_id, loc);
    return it->second;
}

bool LLFontGpuGlyphCache::ensureGLBuffer() {
    if (!glTexBuffer) return false;

    if (!sGLBuffer) {
        glGenBuffers(1, &sGLBuffer);
        glGenTextures(1, &sGLTexture);
        sGLCapacityBytes = 0;
        sUploadedBytes = 0;
    }

    U32 want = static_cast<U32>(llmax((U64)sMaxTexels * kBytesPerTexel, (U64)sArena.size()));
    if (sGLCapacityBytes < want) {
        glBindBuffer(GL_TEXTURE_BUFFER, sGLBuffer);
        glBufferData(GL_TEXTURE_BUFFER, want, nullptr, GL_DYNAMIC_DRAW);
        sGLCapacityBytes = want;
        sUploadedBytes = 0;
        glBindTexture(GL_TEXTURE_BUFFER, sGLTexture);
        glTexBuffer(GL_TEXTURE_BUFFER, GL_RGBA16I, sGLBuffer);
    }
    return true;
}

bool LLFontGpuGlyphCache::bindBufferTexture() {
    if (!ensureGLBuffer()) return false;

    if (sUploadedBytes < sArena.size()) {
        glBindBuffer(GL_TEXTURE_BUFFER, sGLBuffer);
        glBufferSubData(GL_TEXTURE_BUFFER, sUploadedBytes, sArena.size() - sUploadedBytes, sArena.data() + sUploadedBytes);
        sUploadedBytes = static_cast<U32>(sArena.size());
    }

    glBindTexture(GL_TEXTURE_BUFFER, sGLTexture);
    return true;
}

void LLFontGpuGlyphCache::destroyGL() {
    if (sGLTexture) {
        glDeleteTextures(1, &sGLTexture);
        sGLTexture = 0;
    }
    if (sGLBuffer) {
        glDeleteBuffers(1, &sGLBuffer);
        sGLBuffer = 0;
    }
    sGLCapacityBytes = 0;
    sUploadedBytes = 0;
}

void LLFontGpuGlyphCache::reset() {
    llassert(sActiveBatches == 0);
    sArena.clear();
    sUploadedBytes = 0;
    sResetPending = false;
    ++sGeneration;
}
#endif // LL_HAS_HB_GPU
