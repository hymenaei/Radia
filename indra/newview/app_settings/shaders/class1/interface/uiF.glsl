/**
 * @file uiF.glsl
 *
 * $LicenseInfo:firstyear=2007&license=viewerlgpl$
 * Second Life Viewer Source Code
 * Copyright (C) 2007, Linden Research, Inc.
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
 *
 * Linden Research, Inc., 945 Battery Street, San Francisco, CA  94111  USA
 * $/LicenseInfo$
 */

out vec4 frag_color;

uniform sampler2D diffuseMap;

uniform int shadowMode;

in vec2 vary_texcoord0;
in vec4 vertex_color;

#ifdef HAS_FONT_GPU
flat in uint vary_glyphLoc;
#endif

float sampleAtlasAlpha(vec2 uv) {
    return texture(diffuseMap, uv).a;
}

void main() {
#ifdef HAS_FONT_GPU
    if (vary_glyphLoc != 0xFFFFFFFFu) {
        if ((vary_glyphLoc & 0x80000000u) != 0u) {
            float coverage;
            vec4 premul = hb_gpu_paint(vary_texcoord0, vary_glyphLoc & 0x3FFFFFFFu, vec4(vertex_color.rgb, 1.0), coverage);
            if ((vary_glyphLoc & 0x40000000u) != 0u) {
                frag_color = vec4(vertex_color.rgb, premul.a * vertex_color.a);
                return;
            }
            frag_color = vec4(premul.rgb / max(premul.a, 1e-5), premul.a * vertex_color.a);
            return;
        }

        float coverage = hb_gpu_draw(vary_texcoord0, vary_glyphLoc);
        float ppem = hb_gpu_ppem(vary_texcoord0, vary_glyphLoc);
        coverage = alchemy_font_refine_coverage(coverage, vertex_color.rgb, ppem);
        frag_color = vec4(vertex_color.rgb, vertex_color.a * coverage);
        return;
    }
#endif
    if (shadowMode == 0) {
        frag_color = vertex_color * texture(diffuseMap, vary_texcoord0.xy);
        return;
    }

    vec2 atlasTexelSize = 1.0 / vec2(textureSize(diffuseMap, 0));
    float vc_a = vertex_color.a;
    float p;
    if (shadowMode == 1) {
        p = 1.0 - vc_a * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(-1.0, 1.0));
    } else {
        p = (1.0 - vc_a * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(1.0, 1.0)));
        p *= (1.0 - vc_a * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(-1.0, 1.0)));
        p *= (1.0 - vc_a * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(-1.0, -1.0)));
        p *= (1.0 - vc_a * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(1.0, -1.0)));
        p *= (1.0 - vc_a * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(0.0, 2.0)));
    }
    frag_color = vec4(vertex_color.rgb, 1.0 - p);
}
