/**
 * @file llfontgpubatch_test.cpp
 * @brief Tests glyph-quad batching and HarfBuzz glyph placement for the analytic font path.
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
#include "../llfontgpubatch.h"
#include "../llfontgpuglyphcache.h"
#include "../llvertexbuffer.h"
#include "../test/lltut.h"
#if LL_HAS_HB_GPU
    #include <cmath>
    #include <cstdio>
    #include <string>
    #include <hb.h>

namespace {
    #ifndef LLFONT_TEST_DATA_DIR
        #define LLFONT_TEST_DATA_DIR ""
    #endif
constexpr const char* kFontDir = LLFONT_TEST_DATA_DIR;
constexpr const char* kOutlineFont = "IBMPlexMono-Regular.ttf";

bool fileExists(const std::string& path) {
    if (FILE* f = std::fopen(path.c_str(), "rb")) {
        std::fclose(f);
        return true;
    }
    return false;
}

struct TestFace {
    hb_blob_t* blob = nullptr;
    hb_face_t* face = nullptr;
    hb_font_t* font = nullptr;
    TestFace() {
        const std::string path = std::string(kFontDir) + kOutlineFont;
        if (!fileExists(path)) return;
        blob = hb_blob_create_from_file(path.c_str());
        if (hb_blob_get_length(blob) > 0) {
            face = hb_face_create(blob, 0);
            font = hb_font_create(face);
        }
    }
    ~TestFace() {
        if (font) hb_font_destroy(font);
        if (face) hb_face_destroy(face);
        if (blob) hb_blob_destroy(blob);
    }
    bool valid() const { return face != nullptr; }
};

U32 gidFor(hb_face_t* face, hb_codepoint_t cp) {
    hb_font_t* f = hb_font_create(face);
    hb_codepoint_t g = 0;
    hb_font_get_nominal_glyph(f, cp, &g);
    hb_font_destroy(f);
    return (U32)g;
}

bool approx(F32 a, F32 b, F32 eps = 0.05f) {
    return std::fabs(a - b) <= eps;
}
} // namespace

namespace tut {
struct llfontgpubatch_data {};

typedef test_group<llfontgpubatch_data> llfontgpubatch_test;
typedef llfontgpubatch_test::object llfontgpubatch_object;
tut::llfontgpubatch_test llfontgpubatch_testcase("LLFontGpuBatch");

template<> template<> void llfontgpubatch_object::test<1>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);
    auto batch = LLFontGpuGlyphCache::beginBatch();

    const U32 gid_A = gidFor(tf.face, (hb_codepoint_t)'A');
    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getGlyph(batch, gid_A);
    ensure("'A' drawable", loc.drawable());

    const F32 scale = 0.05f;
    const F32 pen_x = 100.f, pen_y = 80.f;
    const LLColor4U color(10, 20, 30, 255);
    const U32 glyph_loc = loc.mTexelOffset;

    LLVector4a pos[6];
    LLVector2 uv[6];
    LLColor4U col[6];
    U32 gl[6];
    LLFontGpuBatch::buildGlyphQuad(pos, uv, col, gl, loc, pen_x, pen_y, scale, 0.f, color, glyph_loc);

    F32 min_u = 1e9f, max_u = -1e9f, min_v = 1e9f, max_v = -1e9f;
    for (S32 i = 0; i < 6; ++i) {
        const F32* p = pos[i].getF32ptr();

        ensure("pos.x uses uniform scale", approx(p[0], pen_x + uv[i].mV[0] * scale));
        ensure("pos.y uses uniform scale", approx(p[1], pen_y + uv[i].mV[1] * scale));
        ensure_equals("glyph_loc matches", gl[i], glyph_loc);
        ensure("color preserved", col[i].mV[0] == 10 && col[i].mV[1] == 20 && col[i].mV[2] == 30 && col[i].mV[3] == 255);

        min_u = std::min(min_u, uv[i].mV[0]);
        max_u = std::max(max_u, uv[i].mV[0]);
        min_v = std::min(min_v, uv[i].mV[1]);
        max_v = std::max(max_v, uv[i].mV[1]);
    }

    ensure("tc x-span >= width", (max_u - min_u) >= (F32)std::abs(loc.mWidth));
    ensure("tc y-span >= |height|", (max_v - min_v) >= (F32)std::abs(loc.mHeight));
}

template<> template<> void llfontgpubatch_object::test<2>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);
    auto batch = LLFontGpuGlyphCache::beginBatch();
    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getGlyph(batch, gidFor(tf.face, (hb_codepoint_t)'A'));
    ensure("'A' drawable", loc.drawable());

    const U32 glyph_loc =
        (loc.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK) | LLVertexBuffer::GLYPH_LOC_COLOR | LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK;
    LLVector4a pos[6];
    LLVector2 uv[6];
    LLColor4U col[6];
    U32 gl[6];
    LLFontGpuBatch::buildGlyphQuad(pos, uv, col, gl, loc, 0.f, 0.f, 0.05f, 0.f, LLColor4U::white, glyph_loc);
    for (S32 i = 0; i < 6; ++i) {
        ensure("COLOR bit set", (gl[i] & LLVertexBuffer::GLYPH_LOC_COLOR) != 0u);
        ensure("color mask bit set", (gl[i] & LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK) != 0u);
        ensure_equals("offset recoverable", gl[i] & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK, loc.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK);
    }
}
} // namespace tut
#endif // LL_HAS_HB_GPU
