/**
 * @file llfontgpushader_test.cpp
 * @brief Tests generated HarfBuzz-GPU shader sources and their analytic text-rendering features.
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
#include "../llfontgpushader.h"
#include "../test/lltut.h"
#if LL_HAS_HB_GPU
    #include "../llfontgpubatch.h"
    #include "../llfontgpuglyphcache.h"
    #include "../llshadermgr.h"
    #include "../llvertexbuffer.h"
    #include <hb.h>
    #include <string>
    #include <vector>
    #if LL_MESA_HEADLESS
        #include "../llglslshader.h"
        #include "llheadlessgl_fixture.h"
    #endif

namespace {
bool contains(const std::string& haystack, const char* needle) {
    return haystack.find(needle) != std::string::npos;
}

#if LL_MESA_HEADLESS
    #ifndef LLFONT_TEST_DATA_DIR
        #define LLFONT_TEST_DATA_DIR ""
    #endif

constexpr const char* kFontDir = LLFONT_TEST_DATA_DIR;

struct TestFont {
    hb_blob_t* blob = nullptr;
    hb_face_t* face = nullptr;
    hb_font_t* font = nullptr;

    explicit TestFont(const char* fontName) {
        const std::string path = std::string(kFontDir) + fontName;
        blob = hb_blob_create_from_file(path.c_str());
        if (!blob || hb_blob_get_length(blob) == 0) return;

        face = hb_face_create(blob, 0);
        font = hb_font_create(face);
    }

    ~TestFont() {
        if (font) hb_font_destroy(font);
        if (face) hb_face_destroy(face);
        if (blob) hb_blob_destroy(blob);
    }

    bool valid() const { return font != nullptr && hb_face_get_glyph_count(face) != 0; }
};

U32 glyphFor(hb_font_t* font, hb_codepoint_t codepoint) {
    hb_codepoint_t glyph = 0;
    return hb_font_get_nominal_glyph(font, codepoint, &glyph) ? static_cast<U32>(glyph) : 0;
}

bool hasDominantChannel(const std::vector<U8>& pixels, S32 xBegin, S32 xEnd, U32 channel) {
    for (S32 y = 0; y < ll_test::HeadlessGL::HEIGHT; ++y) {
        for (S32 x = xBegin; x < xEnd; ++x) {
            const std::size_t offset = (static_cast<std::size_t>(y) * ll_test::HeadlessGL::WIDTH + x) * 4;
            const int dominant = pixels[offset + channel];
            const int otherA = pixels[offset + ((channel + 1) % 3)];
            const int otherB = pixels[offset + ((channel + 2) % 3)];
            if (dominant >= 32 && dominant > otherA + 20 && dominant > otherB + 20) return true;
        }
    }
    return false;
}

void drawGlyph(const LLFontGpuGlyphCache::GlyphLoc& location, F32 penX, const LLColor4U& color, U32 glyphLocation) {
    LLVector4a positions[6];
    LLVector2 texcoords[6];
    LLColor4U colors[6];
    U32 glyphLocations[6];
    LLFontGpuBatch::buildGlyphQuad(positions, texcoords, colors, glyphLocations, location, penX, 80.f, 0.05f, 0.f, color, glyphLocation);

    gGL.begin(LLRender::TRIANGLES);
    gGL.vertexBatchPreTransformed(positions, texcoords, colors, glyphLocations, 6);
    gGL.end();
}
#endif
} // namespace

namespace tut {
struct llfontgpushader_data {};

typedef test_group<llfontgpushader_data> llfontgpushader_test;
typedef llfontgpushader_test::object llfontgpushader_object;
tut::llfontgpushader_test llfontgpushader_testcase("LLFontGpuShader");

template<> template<> void llfontgpushader_object::test<1>() {
    const std::string library = LLFontGpuShader::fragmentLibSource();

    ensure("library has no version directive", !contains(library, "#version"));

    ensure("library has shared rasterizer", contains(library, "_hb_gpu_slug"));
    ensure("library has glyph atlas", contains(library, "hb_gpu_atlas"));
    ensure("library has outline renderer", contains(library, "hb_gpu_draw"));
    ensure("library has color renderer", contains(library, "hb_gpu_paint"));
    ensure("multisample antialiasing remains enabled", !contains(library, "#define HB_GPU_NO_MSAA"));
    ensure("coverage refinement is production library code", contains(library, "alchemy_font_refine_coverage"));
    ensure("coverage refinement keeps stem darkening", contains(library, "coverage = hb_gpu_stem_darken"));

    const bool hasSmoothstep = contains(library, "smoothstep(0.03, 0.97, coverage)");
    const bool hasSmallText = contains(library, "0.20 * small_text");
    ensure("coverage refinement remains mild", hasSmoothstep && hasSmallText);

    const auto slug = library.find("_hb_gpu_slug");
    const auto draw = library.find("float hb_gpu_draw");
    const auto paint = library.find("vec4 hb_gpu_paint");
    ensure("shared precedes draw", slug < draw);
    ensure("draw precedes paint", draw < paint);
}

    #if LL_MESA_HEADLESS
inline ll_test::HeadlessGL& getSharedHeadlessGL() {
    static ll_test::HeadlessGL sHeadlessGl(true, true, true, true);
    return sHeadlessGl;
}

template<> template<> void llfontgpushader_object::test<2>() {
    getSharedHeadlessGL();

    LLGLSLShader* program = LLFontGpuShader::getBatchedProgram();
    ensure("production batched program built",
           program != nullptr);
    if (!program) return;
    ensure("production batched program linked", program->isComplete());
    ensure("Matrices block resolves", glGetUniformBlockIndex(program->mProgramObject, "Matrices") != GL_INVALID_INDEX);
    GLint matricesBinding = -1;
    const GLuint matricesBlock = glGetUniformBlockIndex(program->mProgramObject, "Matrices");
    glGetActiveUniformBlockiv(program->mProgramObject, matricesBlock, GL_UNIFORM_BLOCK_BINDING, &matricesBinding);
    ensure_equals("Matrices uses UB_MATRICES", matricesBinding, static_cast<GLint>(LLGLSLShader::UB_MATRICES));
    ensure("MVP is not a loose uniform", program->getUniformLocation(LLStaticHashedString("modelview_projection_matrix")) < 0);
    ensure("glyph atlas resolves", program->getUniformLocation(LLStaticHashedString("hb_gpu_atlas")) >= 0);
    LLFontGpuShader::destroyBatchedProgram();
}

template<> template<> void llfontgpushader_object::test<3>() {
    getSharedHeadlessGL();
    if (!LLFontGpuShader::isRuntimeSupported()) skip("headless context lacks texture-buffer support");

    TestFont outlineFont("IBMPlexMono-Regular.ttf");
    TestFont colorFont("Noto-COLRv1.ttf");
    if (!outlineFont.valid() || !colorFont.valid()) skip("analytic font rendering test fonts are not present");

    const U32 outlineGlyph = glyphFor(outlineFont.font, static_cast<hb_codepoint_t>('A'));
    const U32 colorGlyph = glyphFor(colorFont.font, static_cast<hb_codepoint_t>(0x2764));
    ensure("outline glyph resolves", outlineGlyph != 0);
    ensure("color glyph resolves", colorGlyph != 0);

    LLFontGpuGlyphCache::resetForTesting();
    LLFontGpuGlyphCache outlineCache;
    LLFontGpuGlyphCache colorCache;
    outlineCache.init(outlineFont.font);
    colorCache.init(colorFont.font, true);

    auto batch = LLFontGpuGlyphCache::beginBatch();
    const LLFontGpuGlyphCache::GlyphLoc outlineLocation = outlineCache.getOrEncodeGlyph(batch, outlineGlyph);
    const LLFontGpuGlyphCache::GlyphLoc colorLocation = colorCache.getOrEncodeGlyph(batch, colorGlyph);
    ensure("outline glyph encodes", outlineLocation.drawable());
    ensure("color glyph encodes", colorLocation.drawable());

    LLGLSLShader* program = LLFontGpuShader::getBatchedProgram();
    ensure("production batched program built",
           program != nullptr);
    if (!program) return;
    program->bind();

    const S32 glyphUnit = program->getTextureChannel(LLShaderMgr::FONT_GLYPH_BUFFER);
    ensure("glyph buffer channel resolves", glyphUnit >= 0);
    ensure("glyph buffer binds", LLFontGpuGlyphCache::bindBufferTexture(static_cast<U32>(glyphUnit)));

    getSharedHeadlessGL().clearFramebuffer();
    gGL.setSceneBlendType(LLRender::BT_ALPHA);

    drawGlyph(outlineLocation, 24.f, LLColor4U(20, 70, 230, 255), outlineLocation.mTexelOffset);
    drawGlyph(colorLocation, 100.f, LLColor4U::white,
              (colorLocation.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK) | LLVertexBuffer::GLYPH_LOC_COLOR);
    drawGlyph(colorLocation, 176.f, LLColor4U(20, 220, 40, 255),
              (colorLocation.mTexelOffset & LLVertexBuffer::GLYPH_LOC_OFFSET_MASK) |
                  LLVertexBuffer::GLYPH_LOC_COLOR |
                  LLVertexBuffer::GLYPH_LOC_COLOR_AS_MASK);
    gGL.flush();

    const std::vector<U8> pixels = ll_test::readFramebufferRGBA(ll_test::HeadlessGL::WIDTH, ll_test::HeadlessGL::HEIGHT);
    ensure("monochrome path produces colored coverage", hasDominantChannel(pixels, 0, 96, 2));
    ensure("color path produces painted pixels", hasDominantChannel(pixels, 80, 168, 0));
    ensure("color-as-mask path uses vertex color", hasDominantChannel(pixels, 160, 256, 1));

    ll_test::gUIProgram.bind();
    LLFontGpuShader::destroyBatchedProgram();
    LLFontGpuGlyphCache::destroyGL();
}
    #endif
} // namespace tut
#endif // LL_HAS_HB_GPU
