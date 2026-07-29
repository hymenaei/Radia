/**
 * @file llfontgpushader_test.cpp
 * @brief Tests the exact analytic shader sources used in production.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Alchemy Viewer Source Code
 * $/LicenseInfo$
 */

#include "linden_common.h"

#include "../test/lltut.h"
#include "../llfontgpushader.h"

#if LL_HAS_HB_GPU

#include <string>

#if LL_MESA_HEADLESS
#  include "../llglslshader.h"
#  include "llheadlessgl_fixture.h"
#endif

namespace
{
    bool contains(const std::string& haystack, const char* needle)
    {
        return haystack.find(needle) != std::string::npos;
    }
}

namespace tut
{
    struct llfontgpushader_data {};

    typedef test_group<llfontgpushader_data> llfontgpushader_test;
    typedef llfontgpushader_test::object llfontgpushader_object;
    tut::llfontgpushader_test llfontgpushader_testcase("LLFontGpuShader");

    // Source inspection covers the exact HUD/world/debug program. The same
    // fragment library is injected into the normal UI shader.
    template<> template<>
    void llfontgpushader_object::test<1>()
    {
        const std::string library = LLFontGpuShader::fragmentLibSource();
        const std::string vertex = LLFontGpuShader::batchedVertexSource();
        const std::string fragment = LLFontGpuShader::batchedFragmentSource();

        ensure("vertex starts with #version", vertex.rfind("#version", 0) == 0);
        ensure("fragment starts with #version", fragment.rfind("#version", 0) == 0);
        ensure("library has no version directive",
               !contains(library, "#version"));

        ensure("vertex declares MVP", contains(vertex, "modelview_projection_matrix"));
        ensure("vertex declares position", contains(vertex, "in vec3 position"));
        ensure("vertex declares texcoord", contains(vertex, "in vec2 texcoord0"));
        ensure("vertex declares glyph location", contains(vertex, "in uint glyph_loc"));

        ensure("library has shared rasterizer", contains(library, "_hb_gpu_slug"));
        ensure("library has glyph atlas", contains(library, "hb_gpu_atlas"));
        ensure("library has outline renderer", contains(library, "hb_gpu_draw"));
        ensure("library has color renderer", contains(library, "hb_gpu_paint"));
        ensure("multisample antialiasing remains enabled",
               !contains(library, "#define HB_GPU_NO_MSAA"));
        ensure("coverage refinement is production library code",
               contains(library, "alchemy_font_refine_coverage"));
        ensure("coverage refinement keeps stem darkening",
               contains(library, "coverage = hb_gpu_stem_darken"));
        ensure("coverage refinement remains mild",
               contains(library, "smoothstep(0.03, 0.97, coverage)") &&
               contains(library, "0.20 * small_text"));

        const auto slug = library.find("_hb_gpu_slug");
        const auto draw = library.find("float hb_gpu_draw");
        const auto paint = library.find("vec4 hb_gpu_paint");
        ensure("shared precedes draw", slug < draw);
        ensure("draw precedes paint", draw < paint);

        ensure("batched fragment embeds production library",
               fragment.find(library) != std::string::npos);
        ensure("batched main dispatches color glyphs",
               contains(fragment, "vary_glyphLoc & 0x80000000u"));
        ensure("batched main supports color coverage masks for shadows",
               contains(fragment, "vary_glyphLoc & 0x40000000u"));
        ensure("batched main draws monochrome glyphs",
               contains(fragment, "hb_gpu_draw(vary_renderCoord"));
    }

#if LL_MESA_HEADLESS
    inline ll_test::HeadlessGL& getSharedHeadlessGL()
    {
        static ll_test::HeadlessGL gl(
            /*needs_vbos=*/true, /*needs_imagegl=*/true,
            /*needs_llrender=*/true, /*needs_render=*/true);
        return gl;
    }

    // Compile and link the exact program getBatchedProgram() builds.
    template<> template<>
    void llfontgpushader_object::test<2>()
    {
        getSharedHeadlessGL();

        LLGLSLShader program;
        ensure("production batched program built",
               LLFontGpuShader::buildBatchedProgram(program));
        ensure("production batched program linked", program.isComplete());
        ensure("MVP resolves",
               program.getUniformLocation(
                   LLStaticHashedString("modelview_projection_matrix")) >= 0);
        ensure("glyph atlas resolves",
               program.getUniformLocation(
                   LLStaticHashedString("hb_gpu_atlas")) >= 0);
        program.unload();
    }
#endif
}

#endif // LL_HAS_HB_GPU
