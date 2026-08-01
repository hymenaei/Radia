/**
 * @file llfontgpushader.cpp
 * @brief Builds the analytic UI text shader program.
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
#include "llfontgpushader.h"
#if LL_HAS_HB_GPU
    #include "llgl.h"
    #include "llglheaders.h"
    #include "llglslshader.h"

namespace {
const char* COVERAGE_REFINEMENT = R"GLSL(
float alchemy_font_refine_coverage(float coverage, vec3 foreground, float ppem) {
    float brightness = dot(foreground, vec3(0.299, 0.587, 0.114));
    coverage = hb_gpu_stem_darken(coverage, brightness, ppem);

    float small_text = 1.0 - smoothstep(10.0, 24.0, ppem);
    float crisp = smoothstep(0.03, 0.97, coverage);
    return mix(coverage, crisp, 0.20 * small_text);
})GLSL";

const char* BATCHED_VERTEX_MAIN = R"GLSL(
uniform mat4 modelview_projection_matrix;

in vec3 position;
in vec2 texcoord0;
in vec4 diffuse_color;
in uint glyph_loc;

out vec2 vary_renderCoord;
flat out uint vary_glyphLoc;
out vec4 vary_color;

void main() {
    gl_Position      = modelview_projection_matrix * vec4(position, 1.0);
    vary_renderCoord = texcoord0;
    vary_glyphLoc    = glyph_loc;
    vary_color       = diffuse_color;
})GLSL";

const char* BATCHED_FRAGMENT_MAIN = R"GLSL(
in vec2 vary_renderCoord;
flat in uint vary_glyphLoc;
in vec4 vary_color;

out vec4 frag_color;

void main() {
    if ((vary_glyphLoc & 0x80000000u) != 0u) {
        float coverage;
        vec4 premul = hb_gpu_paint(vary_renderCoord, vary_glyphLoc & 0x3FFFFFFFu, vec4(vary_color.rgb, 1.0), coverage);
        if ((vary_glyphLoc & 0x40000000u) != 0u) {
            frag_color = vec4(vary_color.rgb, premul.a * vary_color.a);
            return;
        }
        frag_color = vec4(premul.rgb / max(premul.a, 1e-5), premul.a * vary_color.a);
        return;
    }

    float coverage = hb_gpu_draw(vary_renderCoord, vary_glyphLoc);
    float ppem = hb_gpu_ppem(vary_renderCoord, vary_glyphLoc);
    coverage = alchemy_font_refine_coverage(coverage, vary_color.rgb, ppem);
    frag_color = vec4(vary_color.rgb, vary_color.a * coverage);
})GLSL";

GLuint compileStage(GLenum type, const std::string& src) {
    GLuint sh = glCreateShader(type);
    const char* s = src.c_str();
    glShaderSource(sh, 1, &s, nullptr);
    glCompileShader(sh);

    GLint ok = GL_FALSE;
    glGetShaderiv(sh, GL_COMPILE_STATUS, &ok);
    if (!ok) {
        GLchar log[4096] = {0};
        glGetShaderInfoLog(sh, sizeof(log) - 1, nullptr, log);
        LL_WARNS("FontGpu") << "hb-gpu " << (type == GL_VERTEX_SHADER ? "vertex" : "fragment") << " shader compile failed:\n" << log << LL_ENDL;
        glDeleteShader(sh);
        return 0;
    }
    return sh;
}

LLGLSLShader& batchedProgram() {
    static LLGLSLShader program;
    return program;
}

bool& batchedProgramAttempted() {
    static bool attempted = false;
    return attempted;
}
} // namespace

bool LLFontGpuShader::isRuntimeSupported() {
    return glTexBuffer != nullptr;
}

std::string LLFontGpuShader::fragmentLibSource() {
    std::string src;
    if (const char* shared = hb_gpu_shader_source(HB_GPU_SHADER_STAGE_FRAGMENT, HB_GPU_SHADER_LANG_GLSL)) src += shared;
    src += '\n';
    if (const char* draw = hb_gpu_draw_shader_source(HB_GPU_SHADER_STAGE_FRAGMENT, HB_GPU_SHADER_LANG_GLSL)) src += draw;
    src += '\n';
    if (const char* paint = hb_gpu_paint_shader_source(HB_GPU_SHADER_STAGE_FRAGMENT, HB_GPU_SHADER_LANG_GLSL)) src += paint;
    src += '\n';
    src += COVERAGE_REFINEMENT;
    return src;
}

std::string LLFontGpuShader::batchedVertexSource() {
    std::string src = "#version 330\n";
    src += BATCHED_VERTEX_MAIN;
    return src;
}

std::string LLFontGpuShader::batchedFragmentSource() {
    std::string src = "#version 330\n";
    src += fragmentLibSource();
    src += BATCHED_FRAGMENT_MAIN;
    return src;
}

namespace {
bool buildFromSources(LLGLSLShader& program, const std::string& vs_src, const std::string& fs_src, const char* name) {
    program.unload();

    GLuint vs = compileStage(GL_VERTEX_SHADER, vs_src);
    GLuint fs = compileStage(GL_FRAGMENT_SHADER, fs_src);
    if (!vs || !fs) {
        if (vs) glDeleteShader(vs);
        if (fs) glDeleteShader(fs);
        return false;
    }

    program.mProgramObject = glCreateProgram();
    program.mName = name;
    program.attachObject(vs);
    program.attachObject(fs);

    if (!program.mapAttributes()) {
        glDeleteShader(vs);
        glDeleteShader(fs);
        program.unload();
        program.mProgramObject = 0;
        return false;
    }

    glDeleteShader(vs);
    glDeleteShader(fs);

    if (!program.mapUniforms()) {
        program.unload();
        program.mProgramObject = 0;
        return false;
    }

    return program.isComplete();
}
} // namespace

bool LLFontGpuShader::buildBatchedProgram(LLGLSLShader& program) {
    return buildFromSources(program, batchedVertexSource(), batchedFragmentSource(), "font gpu batched fallback");
}

LLGLSLShader* LLFontGpuShader::getBatchedProgram() {
    if (!isRuntimeSupported()) return nullptr;

    LLGLSLShader& program = batchedProgram();
    bool& attempted = batchedProgramAttempted();
    if (!program.isComplete() && !attempted) {
        attempted = true;
        if (buildBatchedProgram(program)) program.mHasFontGpu = true;
    }
    return program.isComplete() ? &program : nullptr;
}

void LLFontGpuShader::destroyBatchedProgram() {
    LLGLSLShader& program = batchedProgram();
    if (program.mProgramObject) program.unload();
    program.mHasFontGpu = false;
    batchedProgramAttempted() = false;
}
#endif // LL_HAS_HB_GPU
