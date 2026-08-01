/**
 * @file llfontgpu_test.cpp
 * @brief Phase 0 de-risk spike for a HarfBuzz-GPU analytic glyph rendering path.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Rye <rye@alchemyviewer.org>
 *                     Hymenaei
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
#include <hb-gpu.h>
#include <hb.h>
#include "../test/lltut.h"
#if LL_MESA_HEADLESS
    #include "llheadlessgl_fixture.h"
#endif
#include <cstdio>
#include <cstring>
#include <string>

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

#if LL_MESA_HEADLESS
std::string assembleDrawGlsl(hb_gpu_shader_stage_t stage) {
    const char* common = hb_gpu_shader_source(stage, HB_GPU_SHADER_LANG_GLSL);
    const char* draw = hb_gpu_draw_shader_source(stage, HB_GPU_SHADER_LANG_GLSL);

    const bool has_version = (common && std::strstr(common, "#version")) || (draw && std::strstr(draw, "#version"));

    std::string src;
    if (!has_version) src += "#version 330\n";
    if (common) {
        src += common;
        src += '\n';
    }
    if (draw) src += draw;
    return src;
}
#endif // LL_MESA_HEADLESS
} // namespace

namespace tut {
struct llfontgpu_data {};

typedef test_group<llfontgpu_data> llfontgpu_test;
typedef llfontgpu_test::object llfontgpu_object;
tut::llfontgpu_test llfontgpu_testcase("LLFontGpu");

template<> template<> void llfontgpu_object::test<1>() {
    hb_gpu_draw_t* draw = hb_gpu_draw_create_or_fail();
    ensure("hb_gpu_draw_create_or_fail returned an encoder", draw != nullptr);
    hb_gpu_draw_destroy(draw);
}

template<> template<> void llfontgpu_object::test<2>() {
    const hb_gpu_shader_stage_t stages[] = {HB_GPU_SHADER_STAGE_VERTEX, HB_GPU_SHADER_STAGE_FRAGMENT};

    for (hb_gpu_shader_stage_t stage : stages) {
        const char* common = hb_gpu_shader_source(stage, HB_GPU_SHADER_LANG_GLSL);
        const char* draw = hb_gpu_draw_shader_source(stage, HB_GPU_SHADER_LANG_GLSL);

        ensure("hb_gpu_shader_source(GLSL) non-null", common != nullptr);
        ensure("hb_gpu_shader_source(GLSL) non-empty", common && common[0] != '\0');
        ensure("hb_gpu_draw_shader_source(GLSL) non-null", draw != nullptr);

        LL_INFOS("FontGpuSpike")
            << "stage="
            << (stage == HB_GPU_SHADER_STAGE_VERTEX ? "VERTEX" : "FRAGMENT")
            << " common_len="
            << (common ? std::strlen(common) : 0)
            << " draw_len="
            << (draw ? std::strlen(draw) : 0)
            << "\n---- common ----\n"
            << (common ? common : "(null)")
            << "\n---- draw ----\n"
            << (draw ? draw : "(null)")
            << LL_ENDL;
    }

    const char* draw_frag = hb_gpu_draw_shader_source(HB_GPU_SHADER_STAGE_FRAGMENT, HB_GPU_SHADER_LANG_GLSL);
    const char* draw_vert = hb_gpu_draw_shader_source(HB_GPU_SHADER_STAGE_VERTEX, HB_GPU_SHADER_LANG_GLSL);
    ensure("draw fragment source provides the rasterizer", draw_frag && draw_frag[0] != '\0');
    ensure("draw vertex source is empty (fragment-only by design)", draw_vert && draw_vert[0] == '\0');
}

template<> template<> void llfontgpu_object::test<3>() {
    const std::string path = std::string(kFontDir) + kOutlineFont;
    if (!fileExists(path)) skip("outline test font not present in test data dir");

    hb_blob_t* blob = hb_blob_create_from_file(path.c_str());
    ensure("font blob loaded", hb_blob_get_length(blob) > 0);

    hb_face_t* face = hb_face_create(blob, 0);
    const unsigned upem = hb_face_get_upem(face);
    ensure("face has a sane upem", upem > 0);

    hb_font_t* font = hb_font_create(face);
    hb_font_set_scale(font, (int)upem, (int)upem);

    hb_codepoint_t gid = 0;
    ensure("cmap maps 'A' to a glyph", hb_font_get_nominal_glyph(font, (hb_codepoint_t)'A', &gid) && gid != 0);

    hb_gpu_draw_t* draw = hb_gpu_draw_create_or_fail();
    ensure("encoder created", draw != nullptr);

    hb_gpu_draw_glyph(draw, font, gid);

    hb_glyph_extents_t extents = {0, 0, 0, 0};
    hb_blob_t* encoded = hb_gpu_draw_encode(draw, &extents);

    ensure("encode produced a blob", encoded != nullptr);
    ensure("encoded blob is non-empty", encoded && hb_blob_get_length(encoded) > 0);
    ensure("glyph has non-zero width", extents.width != 0);
    ensure("glyph has non-zero height", extents.height != 0);

    LL_INFOS("FontGpuSpike")
        << "encoded 'A' gid="
        << gid
        << " blob_bytes="
        << (encoded ? hb_blob_get_length(encoded) : 0)
        << " extents=("
        << extents.x_bearing
        << ","
        << extents.y_bearing
        << " "
        << extents.width
        << "x"
        << extents.height
        << ")"
        << " upem="
        << upem
        << LL_ENDL;

    if (encoded) hb_gpu_draw_recycle_blob(draw, encoded);
    hb_gpu_draw_destroy(draw);
    hb_font_destroy(font);
    hb_face_destroy(face);
    hb_blob_destroy(blob);
}

#if LL_MESA_HEADLESS
inline ll_test::HeadlessGL& getSharedHeadlessGL() {
    static ll_test::HeadlessGL gl(false, false, false, false);
    return gl;
}

template<> template<> void llfontgpu_object::test<4>() {
    getSharedHeadlessGL();

    const std::string vsrc = assembleDrawGlsl(HB_GPU_SHADER_STAGE_VERTEX);
    const std::string fsrc = assembleDrawGlsl(HB_GPU_SHADER_STAGE_FRAGMENT);

    LL_INFOS("FontGpuSpike") << "assembled VERTEX GLSL:\n" << vsrc << LL_ENDL;
    LL_INFOS("FontGpuSpike") << "assembled FRAGMENT GLSL:\n" << fsrc << LL_ENDL;

    GLuint vs = ll_test::compileTestShader(GL_VERTEX_SHADER, vsrc.c_str());
    ensure("hb-gpu vertex GLSL compiled under GL 3.3 core", vs != 0);
    if (vs) glDeleteShader(vs);

    GLuint fs = ll_test::compileTestShader(GL_FRAGMENT_SHADER, fsrc.c_str());
    ensure("hb-gpu fragment GLSL compiled under GL 3.3 core", fs != 0);
    if (fs) glDeleteShader(fs);
}
#endif // LL_MESA_HEADLESS
} // namespace tut
