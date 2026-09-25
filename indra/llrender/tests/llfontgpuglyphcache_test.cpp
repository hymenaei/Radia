/**
 * @file llfontgpuglyphcache_test.cpp
 * @brief Tests atlas-free glyph caching, metrics, color, and variable-font handling for the analytic font path.
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
#include "../llfontgpuglyphcache.h"
#include "../test/lltut.h"
#if LL_HAS_HB_GPU
    #include <hb-ot.h>
    #include <hb.h>
    #include "../alfontface.h"
    #include "../llfontfreetype.h"
    #include "../llfontregistry.h"
    #if LL_MESA_HEADLESS
        #include "llheadlessgl_fixture.h"
    #endif
    #include <cstdio>
    #include <string>

namespace {
    #ifndef LLFONT_TEST_DATA_DIR
        #define LLFONT_TEST_DATA_DIR ""
    #endif
constexpr const char* kFontDir = LLFONT_TEST_DATA_DIR;

constexpr const char* kOutlineFont = "IBMPlexMono-Regular.ttf";
constexpr const char* kColorFont = "Noto-COLRv1.ttf";
constexpr const char* kVarFont = "IBMPlexSansVar-Roman.ttf";

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

    explicit TestFace(const char* fontName = kOutlineFont) {
        LLFontGpuGlyphCache::resetForTesting();
        LLFontGpuGlyphCache::setMaxTexels(1u << 18);

        const std::string path = std::string(kFontDir) + fontName;
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
    return static_cast<U32>(g);
}
} // namespace

namespace tut {
struct llfontgpuglyphcache_data {};

typedef test_group<llfontgpuglyphcache_data> llfontgpuglyphcache_test;
typedef llfontgpuglyphcache_test::object llfontgpuglyphcache_object;
tut::llfontgpuglyphcache_test llfontgpuglyphcache_testcase("LLFontGpuGlyphCache");

template<> template<> void llfontgpuglyphcache_object::test<1>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");

    const U32 glyphIdA = gidFor(tf.face, (hb_codepoint_t)'A');
    ensure("cmap maps 'A'", glyphIdA != 0);

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);
    auto batch = LLFontGpuGlyphCache::beginBatch();

    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getOrEncodeGlyph(batch, glyphIdA);
    ensure("'A' is drawable", loc.drawable());
    ensure("'A' has texels", loc.mTexelCount > 0);
    ensure("first glyph lands at offset 0", loc.mTexelOffset == 0);
    ensure("'A' has non-zero extents", loc.mWidth != 0 && loc.mHeight != 0);
    ensure_equals("one glyph cached", cache.getGlyphCount(), 1U);

    const U32 texelsAfterFirst = cache.getArenaTexels();
    const LLFontGpuGlyphCache::GlyphLoc loc2 = cache.getOrEncodeGlyph(batch, glyphIdA);
    ensure_equals("cache hit: same offset", loc2.mTexelOffset, loc.mTexelOffset);
    ensure_equals("cache hit: same count", loc2.mTexelCount, loc.mTexelCount);
    ensure_equals("cache hit: no arena growth", cache.getArenaTexels(), texelsAfterFirst);
    ensure_equals("cache hit: still one glyph", cache.getGlyphCount(), 1U);
}

template<> template<> void llfontgpuglyphcache_object::test<2>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");

    const U32 glyphIdSpace = gidFor(tf.face, (hb_codepoint_t)' ');
    ensure("cmap maps space", glyphIdSpace != 0);

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);
    auto batch = LLFontGpuGlyphCache::beginBatch();

    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getOrEncodeGlyph(batch, glyphIdSpace);
    const U32 texelsAfterFirst = cache.getArenaTexels();

    const LLFontGpuGlyphCache::GlyphLoc loc2 = cache.getOrEncodeGlyph(batch, glyphIdSpace);
    ensure_equals("space cached: same offset", loc2.mTexelOffset, loc.mTexelOffset);
    ensure_equals("space cached: same count", loc2.mTexelCount, loc.mTexelCount);
    ensure_equals("space cached: no arena growth", cache.getArenaTexels(), texelsAfterFirst);
    ensure_equals("space counts as one cache entry", cache.getGlyphCount(), 1U);
}

template<> template<> void llfontgpuglyphcache_object::test<3>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");

    const U32 glyphIdA = gidFor(tf.face, (hb_codepoint_t)'A');
    const U32 glyphIdB = gidFor(tf.face, (hb_codepoint_t)'B');
    ensure("cmap maps 'A' and 'B'", glyphIdA != 0 && glyphIdB != 0 && glyphIdA != glyphIdB);

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);

    const S32 generationBefore = cache.getGeneration();
    {
        auto batch = LLFontGpuGlyphCache::beginBatch();
        const LLFontGpuGlyphCache::GlyphLoc a = cache.getOrEncodeGlyph(batch, glyphIdA);
        ensure("'A' drawable", a.drawable());

        cache.setMaxTexels(a.mTexelCount);
        const LLFontGpuGlyphCache::GlyphLoc b = cache.getOrEncodeGlyph(batch, glyphIdB);
        ensure("'B' drawable", b.drawable());
        ensure_equals("generation pinned for the complete batch", cache.getGeneration(), generationBefore);
        ensure_equals("both current-batch offsets remain cached", cache.getGlyphCount(), 2U);
        ensure("second glyph follows the first in the live arena", b.mTexelOffset >= a.mTexelOffset + a.mTexelCount);
    }

    auto nextBatch = LLFontGpuGlyphCache::beginBatch();
    ensure("next batch reset bumped generation", cache.getGeneration() > generationBefore);
    const LLFontGpuGlyphCache::GlyphLoc bAfter = cache.getOrEncodeGlyph(nextBatch, glyphIdB);
    ensure_equals("survivor re-encodes at offset 0", bAfter.mTexelOffset, 0U);
    ensure_equals("stale per-face map was cleared lazily", cache.getGlyphCount(), 1U);
}

template<> template<> void llfontgpuglyphcache_object::test<5>() {
    const std::string path = std::string(kFontDir) + kOutlineFont;
    if (!fileExists(path)) skip("outline test font not present in test data dir");

    LLFontManager::initClass();
    {
        ALFontFaceKey key{path, 0, 14.f, 96.f, 96.f, EFontHinting::DEFAULT, 0};
        LLPointer<ALFontFace> face = gFontManagerp->getOrCreateFace(key);
        ensure("face loaded", face.notNull() && face->isValid());

        LLFontGpuGlyphCache* cache = face->getGpuGlyphCache();
        ensure("face hands out a GPU glyph cache", cache != nullptr);
        ensure("GPU cache is stable across calls", face->getGpuGlyphCache() == cache);

        hb_codepoint_t gid = 0;
        ensure("cmap maps 'A'", hb_font_get_nominal_glyph(face->getHbFont(), (hb_codepoint_t)'A', &gid) && gid != 0);
        auto batch = LLFontGpuGlyphCache::beginBatch();
        ensure("face cache encodes 'A'", cache->getOrEncodeGlyph(batch, (U32)gid).drawable());
    }
    LLFontManager::cleanupClass();
}

template<> template<> void llfontgpuglyphcache_object::test<6>() {
    TestFace tf(kColorFont);
    if (!tf.valid()) skip("COLRv1 test font not present in test data dir");

    const U32 gid = gidFor(tf.face, (hb_codepoint_t)0x2764);
    ensure("cmap maps U+2764", gid != 0);

    LLFontGpuGlyphCache cache;
    cache.init(tf.font, true, 0);
    ensure("cache reports color mode", cache.isColor());
    auto batch = LLFontGpuGlyphCache::beginBatch();

    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getOrEncodeGlyph(batch, gid);
    ensure("color glyph is drawable", loc.drawable());
    ensure("color glyph has texels", loc.mTexelCount > 0);
    ensure("first glyph lands at offset 0", loc.mTexelOffset == 0);
    ensure("color glyph has non-zero extents", loc.mWidth != 0 && loc.mHeight != 0);
    ensure_equals("one glyph cached", cache.getGlyphCount(), 1U);

    const U32 texels = cache.getArenaTexels();
    const LLFontGpuGlyphCache::GlyphLoc loc2 = cache.getOrEncodeGlyph(batch, gid);
    ensure_equals("cache hit: same offset", loc2.mTexelOffset, loc.mTexelOffset);
    ensure_equals("cache hit: no growth", cache.getArenaTexels(), texels);
}

template<> template<> void llfontgpuglyphcache_object::test<7>() {
    TestFace tf(kVarFont);
    if (!tf.valid()) skip("variable test font not present in test data dir");
    if (hb_ot_var_get_axis_count(tf.face) == 0) skip("test font is not variable");

    const U32 glyphIdI = gidFor(tf.face, (hb_codepoint_t)'I');
    ensure("cmap maps 'I'", glyphIdI != 0);

    LLFontGpuGlyphCache reg;
    reg.init(tf.font);
    auto batch = LLFontGpuGlyphCache::beginBatch();
    const LLFontGpuGlyphCache::GlyphLoc regularLocation = reg.getOrEncodeGlyph(batch, glyphIdI);
    ensure("regular 'I' drawable", regularLocation.drawable());

    hb_font_t* boldFont = hb_font_create(tf.face);
    hb_font_set_variation(boldFont, HB_TAG('w', 'g', 'h', 't'), 700.f);
    LLFontGpuGlyphCache bold;
    bold.init(boldFont);
    const LLFontGpuGlyphCache::GlyphLoc boldLocation = bold.getOrEncodeGlyph(batch, glyphIdI);
    hb_font_destroy(boldFont);
    ensure("bold 'I' drawable", boldLocation.drawable());

    ensure("bold 'I' stem wider than regular master", boldLocation.mWidth > regularLocation.mWidth);
}

    #if LL_MESA_HEADLESS
inline ll_test::HeadlessGL& getSharedHeadlessGL() {
    static ll_test::HeadlessGL sHeadlessGl(true, true, true, false);
    return sHeadlessGl;
}

template<> template<> void llfontgpuglyphcache_object::test<4>() {
    TestFace tf;
    if (!tf.valid()) skip("outline test font not present in test data dir");
    getSharedHeadlessGL();

    LLFontGpuGlyphCache cache;
    cache.init(tf.font);
    const U32 glyphIdA = gidFor(tf.face, (hb_codepoint_t)'A');
    auto batch = LLFontGpuGlyphCache::beginBatch();
    const LLFontGpuGlyphCache::GlyphLoc loc = cache.getOrEncodeGlyph(batch, glyphIdA);
    ensure("'A' drawable", loc.drawable());

    while (glGetError() != GL_NO_ERROR) {}

    GLint max_texture_units = 0;
    glGetIntegerv(GL_MAX_COMBINED_TEXTURE_IMAGE_UNITS, &max_texture_units);
    if (max_texture_units < 2) skip("headless context has no second texture unit");

    gGL.activateTextureUnit(0);
    ensure("texture unit 0 active before glyph bind", gGL.verifyTexUnitActive(0));

    if (!cache.bindBufferTexture(1)) skip("GL texture buffer unsupported in this context");
    GLint active_texture = 0;
    glGetIntegerv(GL_ACTIVE_TEXTURE, &active_texture);
    ensure_equals("glyph bind restores the caller's active texture unit", active_texture, static_cast<GLint>(GL_TEXTURE0));
    ensure_equals("renderer texture-unit cache stays synchronized", gGL.getCurrentTexUnitIndex(), 0U);

    if (!cache.bindBufferTexture()) skip("GL texture buffer unsupported in this context");
    ensure_equals("no GL error after buffer upload + bind", (int)glGetError(), (int)GL_NO_ERROR);

    ensure("second bind succeeds", cache.bindBufferTexture());
    ensure_equals("no GL error on idempotent bind", (int)glGetError(), (int)GL_NO_ERROR);

    cache.destroyGL();
}
    #endif // LL_MESA_HEADLESS
} // namespace tut
#endif // LL_HAS_HB_GPU
