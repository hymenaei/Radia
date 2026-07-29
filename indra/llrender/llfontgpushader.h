/**
 * @file llfontgpushader.h
 * @brief Builds the analytic (hb-gpu) UI text shader program.
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

#ifndef LL_LLFONTGPUSHADER_H
#define LL_LLFONTGPUSHADER_H

#include "stdtypes.h"
#include "llhbgpu.h"   // LL_HAS_HB_GPU + hb-gpu shader-source API

#if LL_HAS_HB_GPU

#include <string>

class LLGLSLShader;

// Assembles and builds the analytic (hb-gpu) text program. HarfBuzz emits the
// heavy fragment library at runtime; this class exposes the exact sources used
// by the production batched fallback so headless tests compile what ships.
//
// The vertex shader uses the reserved attribute names (position, texcoord0,
// diffuse_color, glyph_loc) so the program binds through LLVertexBuffer.
//
// Lives in llrender rather than LLViewerShaderMgr because the source is
// assembled at runtime, not loaded from .glsl files on disk.
class LLFontGpuShader
{
public:
    // Runtime gate shared by shader setup and the standalone fallback. The
    // library may be present at build time while the active GL context lacks
    // texture-buffer support.
    static bool isRuntimeSupported();

    // HarfBuzz fragment library with no #version or main. The normal UI shader
    // injects this exact source into uiF.glsl.
    static std::string fragmentLibSource();

    // Exact production sources and builder used for HUD/world/debug fallback.
    static std::string batchedVertexSource();
    static std::string batchedFragmentSource();
    static bool buildBatchedProgram(LLGLSLShader& program);

    // Shared mixed mono/COLRv1 program for text drawn outside the normal UI
    // shader (HUD/world/debug overlays). Built lazily; callers must flush before
    // binding and before restoring the previous program.
    static LLGLSLShader* getBatchedProgram();
    static void destroyBatchedProgram();
};

#endif // LL_HAS_HB_GPU
#endif // LL_LLFONTGPUSHADER_H
