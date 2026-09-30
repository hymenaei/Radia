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

    const U32 glyphIdA = gidFor(tf.face, (hb_codepoint_t)'A');
    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getOrEncodeGlyph(batch, glyphIdA);
    ensure("'A' drawable", loc.drawable());

    const F32 scale = 0.05f;
    const F32 penX = 100.f, penY = 80.f;
    const LLColor4U color(10, 20, 30, 255);
    const U32 glyphLocation = loc.mTexelOffset;

    LLVector4a positions[6];
    LLVector2 texcoords[6];
    LLColor4U colors[6];
    U32 glyphLocations[6];
    LLFontGpuBatch::buildGlyphQuad(positions, texcoords, colors, glyphLocations, loc, penX, penY, scale, 0.f, color, glyphLocation);

    F32 minU = 1e9f, maxU = -1e9f, minV = 1e9f, maxV = -1e9f;
    for (S32 i = 0; i < 6; ++i) {
        const F32* p = positions[i].getF32ptr();

        ensure("pos.x uses uniform scale", approx(p[0], penX + texcoords[i].mV[0] * scale));
        ensure("pos.y uses uniform scale", approx(p[1], penY + texcoords[i].mV[1] * scale));
        ensure_equals("glyphLocation matches", glyphLocations[i], glyphLocation);
        ensure("color preserved", colors[i].mV[0] == 10 && colors[i].mV[1] == 20 && colors[i].mV[2] == 30 && colors[i].mV[3] == 255);

        minU = std::min(minU, texcoords[i].mV[0]);
        maxU = std::max(maxU, texcoords[i].mV[0]);
        minV = std::min(minV, texcoords[i].mV[1]);
        maxV = std::max(maxV, texcoords[i].mV[1]);
    }

    ensure("tc x-span >= width", (maxU - minU) >= (F32)std::abs(loc.mWidth));
    ensure("tc y-span >= |height|", (maxV - minV) >= (F32)std::abs(loc.mHeight));
}

template<> template<> void llfontgpubatch_object::test<2>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);
    auto batch = LLFontGpuGlyphCache::beginBatch();
    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getOrEncodeGlyph(batch, gidFor(tf.face, (hb_codepoint_t)'A'));
    ensure("'A' drawable", loc.drawable());

    const U32 glyphLocation =
        (loc.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK) | LLVertexBuffer::GLYPH_LOC_COLOR | LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK;
    LLVector4a positions[6];
    LLVector2 texcoords[6];
    LLColor4U colors[6];
    U32 glyphLocations[6];
    LLFontGpuBatch::buildGlyphQuad(positions, texcoords, colors, glyphLocations, loc, 0.f, 0.f, 0.05f, 0.f, LLColor4U::white, glyphLocation);
    for (S32 i = 0; i < 6; ++i) {
        ensure("COLOR bit set", (glyphLocations[i] & LLVertexBuffer::GLYPH_LOC_COLOR) != 0u);
        ensure("color mask bit set", (glyphLocations[i] & LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK) != 0u);
        ensure_equals("offset recoverable", glyphLocations[i] & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK, loc.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK);
    }
}
} // namespace tut
#endif // LL_HAS_HB_GPU
