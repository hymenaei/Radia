/**
 * @file uiF.glsl
 * @brief Fragment stage for the viewer and retained UI paint shader variants.
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

// Transitional source variants share one UI shader pair while the viewer
// migrates from the legacy texture/font contract to retained painting. Once
// every UI caller uses retained painting, remove the legacy #else branch and
// make the retained path the default fragment program.
#ifdef PAINT_SHADER
out vec4 frag_color;

uniform int paintOp;
uniform vec4 shapeRect;
uniform float shapeRadius;
uniform float shapeBorderWidth;
uniform vec4 shapeColor;
uniform vec2 shapeOffset;
uniform int outlineStyle;
uniform vec4 borderWidths;
uniform vec2 topBorderGap;
uniform int gradientKind;
uniform int gradientRepeating;
uniform vec2 gradientStart;
uniform vec2 gradientEnd;
uniform vec2 gradientCenter;
uniform vec2 gradientRadius;
uniform float gradientAngle;
uniform int gradientStopCount;
uniform vec4 gradientColors[8];
uniform float gradientStops[8];
uniform vec2 shadowOffset;
uniform float shadowBlur;
uniform float shadowSpread;
uniform sampler2D diffuseMap;
uniform vec2 effectTextureSize;
uniform vec2 effectBlurAxis;
uniform vec2 effectBlurRadii;
uniform vec2 effectGradientStart;
uniform vec2 effectGradientEnd;
uniform vec4 effectCaptureRect;
uniform vec4 effectMaskRect;
uniform float effectMaskRadius;
uniform int effectRoundedMask;

const int PAINT_OP_DIRECT = 0;
const int PAINT_OP_FILL = 1;
const int PAINT_OP_BORDER = 2;
const int PAINT_OP_GRADIENT = 3;
const int PAINT_OP_OUTER_SHADOW = 4;
const int PAINT_OP_INSET_SHADOW = 5;
const int PAINT_OP_GRADIENT_BORDER = 6;
const int PAINT_OP_BLUR = 7;
const int PAINT_OP_COMPOSITE = 8;

const int GRADIENT_LINEAR = 0;
const int GRADIENT_RADIAL = 1;
const int GRADIENT_CONIC = 2;

const int OUTLINE_SOLID = 0;
const int OUTLINE_DASHED = 1;

in vec4 vertex_color;
in vec2 shape_coord;

float roundedRectDistance(vec2 p, vec2 size, float radius) {
    float r = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    vec2 half_size = size * 0.5;
    vec2 q = abs(p - half_size) - (half_size - vec2(r));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

float coverageFromDistance(float signed_distance) {
    float aa = max(fwidth(signed_distance), 1.0e-4);
    return 1.0 - smoothstep(-aa * 0.5, aa * 0.5, signed_distance);
}

float roundedRectPerimeterCoordinate(vec2 p, vec2 size, float radius) {
    const float half_pi = 1.57079632679;
    const float pi = 3.14159265359;
    const float two_pi = 6.28318530718;
    float r = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    float horizontal = max(size.x - r * 2.0, 0.0);
    float vertical = max(size.y - r * 2.0, 0.0);
    vec2 point = clamp(p, vec2(0.0), size);

    if (r > 0.0 && point.x > size.x - r && point.y < r) {
        float angle = atan(point.y - r, point.x - (size.x - r));
        return horizontal + r * (angle + half_pi);
    }

    if (r > 0.0 && point.x > size.x - r && point.y > size.y - r) {
        float angle = atan(point.y - (size.y - r), point.x - (size.x - r));
        return horizontal + r * half_pi + vertical + r * angle;
    }

    if (r > 0.0 && point.x < r && point.y > size.y - r) {
        float angle = atan(point.y - (size.y - r), point.x - r);
        return horizontal * 2.0 + r * 2.0 * half_pi + vertical + r * (angle - half_pi);
    }

    if (r > 0.0 && point.x < r && point.y < r) {
        float angle = atan(point.y - r, point.x - r);
        if (angle < 0.0) angle += two_pi;
        return horizontal * 2.0 + r * 3.0 * half_pi + vertical * 2.0 + r * (angle - pi);
    }

    float bottom_distance = abs(point.y);
    float right_distance = abs(size.x - point.x);
    float top_distance = abs(size.y - point.y);
    float left_distance = abs(point.x);
    float nearest = min(min(bottom_distance, right_distance), min(top_distance, left_distance));
    if (nearest == bottom_distance) return clamp(point.x - r, 0.0, horizontal);
    if (nearest == right_distance) return horizontal + r * half_pi + clamp(point.y - r, 0.0, vertical);
    if (nearest == top_distance) return horizontal + r * 2.0 * half_pi + vertical + clamp(size.x - r - point.x, 0.0, horizontal);
    return horizontal * 2.0 + r * 3.0 * half_pi + vertical + clamp(size.y - r - point.y, 0.0, vertical);
}

float dashedOutlineCoverage(vec2 local_coord, vec2 size, float radius, float width) {
    float center_offset = width * 0.5;
    vec2 center_size = max(size - vec2(width), vec2(0.0));
    float center_radius = max(radius - center_offset, 0.0);
    float horizontal = max(center_size.x - center_radius * 2.0, 0.0);
    float vertical = max(center_size.y - center_radius * 2.0, 0.0);
    float perimeter = horizontal * 2.0 + vertical * 2.0 + center_radius * 6.28318530718;
    if (perimeter <= 1.0e-4) return 1.0;
    float preferred_period = max(width * 5.0, 1.0);
    float dash_count = max(floor(perimeter / preferred_period + 0.5), 1.0);
    float period = perimeter / dash_count;
    float dash_length = period * 0.6;
    float coordinate = roundedRectPerimeterCoordinate(local_coord - vec2(center_offset), center_size, center_radius);
    float phase = mod(coordinate, period);
    float dash_distance = abs(phase - dash_length * 0.5) - dash_length * 0.5;
    float aa = max(fwidth(coordinate), 1.0e-4);
    return 1.0 - smoothstep(-aa * 0.5, aa * 0.5, dash_distance);
}

vec4 blurredEffectColor(vec2 texture_coord) {
    vec2 pixel_coord = texture_coord * effectTextureSize;
    vec2 gradient_axis = effectGradientEnd - effectGradientStart;
    float amount = clamp(dot(pixel_coord - effectGradientStart, gradient_axis) / max(dot(gradient_axis, gradient_axis), 1.0e-6), 0.0, 1.0);
    float radius = mix(effectBlurRadii.x, effectBlurRadii.y, amount);
    if (radius <= 1.0e-4) return texture(diffuseMap, texture_coord);

    const int max_samples_per_side = 32;
    float samples_per_side = min(ceil(radius), float(max_samples_per_side));
    float sample_step = radius / samples_per_side;
    float sigma = max(radius / 3.0, 0.5);
    float inverse_two_sigma_squared = 0.5 / (sigma * sigma);
    vec2 texel = effectBlurAxis / max(effectTextureSize, vec2(1.0));
    vec4 color = vec4(0.0);
    float total_weight = 0.0;
    for (int sample_index = -max_samples_per_side; sample_index <= max_samples_per_side; ++sample_index) {
        float sample_offset = float(sample_index) * sample_step;
        if (abs(sample_offset) > radius + 1.0e-4) continue;
        float weight = exp(-sample_offset * sample_offset * inverse_two_sigma_squared);
        color += texture(diffuseMap, texture_coord + texel * sample_offset) * weight;
        total_weight += weight;
    }
    return color / max(total_weight, 1.0e-6);
}

vec4 compositedEffectColor(vec2 texture_coord) {
    vec4 color = texture(diffuseMap, texture_coord);
    if (effectRoundedMask != 0) {
        vec2 point = effectCaptureRect.xy + texture_coord * effectCaptureRect.zw;
        float distance = roundedRectDistance(point - effectMaskRect.xy, effectMaskRect.zw, effectMaskRadius);
        float mask = coverageFromDistance(distance);
        return vec4(color.rgb, color.a * mask);
    }

    color.rgb = color.a > 1.0e-6 ? color.rgb / color.a : vec3(0.0);
    return color;
}

float gradientAmount(vec2 local_coord) {
    float amount = 0.0;
    if (gradientKind == GRADIENT_LINEAR) {
        vec2 axis = gradientEnd - gradientStart;
        amount = dot(local_coord - gradientStart, axis) / max(dot(axis, axis), 1.0e-6);
    } else if (gradientKind == GRADIENT_RADIAL) amount = length((local_coord - gradientCenter) / max(gradientRadius, vec2(1.0e-4)));
    else {
        const float tau = 6.28318530718;
        vec2 delta = local_coord - gradientCenter;
        amount = atan(delta.x, delta.y) / tau - gradientAngle / 360.0;
    }

    return amount;
}

float gradientPixelWidth(vec2 local_coord, float amount) {
    if (gradientKind != GRADIENT_CONIC) return fwidth(amount);

    const float tau = 6.28318530718;
    vec2 delta = local_coord - gradientCenter;
    vec2 delta_dx = dFdx(delta);
    vec2 delta_dy = dFdy(delta);
    float radius_squared = dot(delta, delta);
    float footprint_squared = max(dot(delta_dx, delta_dx), dot(delta_dy, delta_dy));
    if (radius_squared <= footprint_squared * 0.25) return 1.0;
    radius_squared = max(radius_squared, 1.0e-6);
    float turn_dx = (delta.y * delta_dx.x - delta.x * delta_dx.y) / (tau * radius_squared);
    float turn_dy = (delta.y * delta_dy.x - delta.x * delta_dy.y) / (tau * radius_squared);
    return abs(turn_dx) + abs(turn_dy);
}

vec4 gradientIntervalIntegral(float amount) {
    float first = gradientStops[0];
    float last = gradientStops[gradientStopCount - 1];
    float limit = clamp(amount, first, last);
    vec4 color = vec4(0.0);
    for (int index = 0; index < 7; ++index) {
        if (index + 1 >= gradientStopCount) break;
        float start = gradientStops[index];
        float width = max(gradientStops[index + 1] - start, 0.0);
        float covered = clamp(limit - start, 0.0, width);
        if (width > 1.0e-6) {
            vec4 slope = (gradientColors[index + 1] - gradientColors[index]) / width;
            color += gradientColors[index] * covered + slope * (covered * covered * 0.5);
        }
    }
    return color;
}

vec4 clampedGradientIntegral(float amount) {
    float first = gradientStops[0];
    float last = gradientStops[gradientStopCount - 1];
    float limit = clamp(amount, 0.0, 1.0);
    vec4 color = gradientColors[0] * min(limit, first);
    color += gradientIntervalIntegral(limit);
    color += gradientColors[gradientStopCount - 1] * max(limit - last, 0.0);
    if (amount < 0.0) color += gradientColors[0] * amount;
    if (amount > 1.0) color += gradientColors[gradientStopCount - 1] * (amount - 1.0);
    return color;
}

vec4 underlyingGradientIntegral(float amount, vec4 repeating_total) {
    if (gradientRepeating == 0) return clampedGradientIntegral(amount);

    float first = gradientStops[0];
    float period = gradientStops[gradientStopCount - 1] - first;
    float cycles = floor((amount - first) / period);
    float wrapped = amount - first - cycles * period;
    return cycles * repeating_total + gradientIntervalIntegral(first + wrapped);
}

vec4 filteredGradientColor(vec2 local_coord) {
    float amount = gradientAmount(local_coord);
    float pixel_width = max(gradientPixelWidth(local_coord, amount), 1.0e-6);
    float start = amount - pixel_width * 0.5;
    float end = amount + pixel_width * 0.5;
    if (gradientKind == GRADIENT_RADIAL) start = max(start, 0.0);

    vec4 repeating_total = vec4(0.0);
    if (gradientRepeating != 0) repeating_total = gradientIntervalIntegral(gradientStops[gradientStopCount - 1]);

    vec4 start_integral;
    vec4 end_integral;
    if (gradientKind == GRADIENT_CONIC) {
        vec4 origin = underlyingGradientIntegral(0.0, repeating_total);
        vec4 turn_total = underlyingGradientIntegral(1.0, repeating_total) - origin;
        float start_turn = floor(start);
        float end_turn = floor(end);
        start_integral = start_turn * turn_total + underlyingGradientIntegral(start - start_turn, repeating_total) - origin;
        end_integral = end_turn * turn_total + underlyingGradientIntegral(end - end_turn, repeating_total) - origin;
    } else {
        start_integral = underlyingGradientIntegral(start, repeating_total);
        end_integral = underlyingGradientIntegral(end, repeating_total);
    }
    return (end_integral - start_integral) / max(end - start, 1.0e-6);
}

void main() {
    if (paintOp == PAINT_OP_DIRECT) {
        frag_color = vertex_color;
        return;
    }

    if (paintOp == PAINT_OP_BLUR) {
        frag_color = blurredEffectColor(shape_coord);
        return;
    }

    if (paintOp == PAINT_OP_COMPOSITE) {
        frag_color = compositedEffectColor(shape_coord);
        return;
    }

    vec2 size = max(shapeRect.zw, vec2(0.0));
    vec2 local_coord = shape_coord - shapeOffset;
    float outer_distance = roundedRectDistance(local_coord, size, shapeRadius);
    float alpha = coverageFromDistance(outer_distance);

    if (paintOp == PAINT_OP_OUTER_SHADOW) {
        float softness = max(shadowBlur, fwidth(outer_distance));
        alpha = 1.0 - smoothstep(-softness, softness, outer_distance);
        frag_color = vec4(shapeColor.rgb, shapeColor.a * alpha);
        return;
    }

    if (paintOp == PAINT_OP_INSET_SHADOW) {
        vec2 hole_size = max(size - vec2(shadowSpread * 2.0), vec2(0.0));
        vec2 hole_coord = local_coord - vec2(shadowSpread) - shadowOffset;
        float hole_radius = max(shapeRadius - shadowSpread, 0.0);
        float hole_distance = roundedRectDistance(hole_coord, hole_size, hole_radius);
        float softness = max(shadowBlur, fwidth(hole_distance));
        alpha *= smoothstep(-softness, softness, hole_distance);
        frag_color = vec4(shapeColor.rgb, shapeColor.a * alpha);
        return;
    }

    if (paintOp == PAINT_OP_BORDER || paintOp == PAINT_OP_GRADIENT_BORDER) {
        vec4 widths = paintOp == PAINT_OP_BORDER ? vec4(shapeBorderWidth) : borderWidths;
        widths = max(widths, vec4(0.0));
        vec2 inner_size = max(size - vec2(widths.w + widths.y, widths.z + widths.x), vec2(0.0));
        vec2 inner_coord = local_coord - vec2(widths.w, widths.z);
        float inner_radius = max(shapeRadius - max(max(widths.x, widths.y), max(widths.z, widths.w)), 0.0);
        if (inner_size.x > 0.0 && inner_size.y > 0.0) {
            float inner_distance = roundedRectDistance(inner_coord, inner_size, inner_radius);
            alpha = coverageFromDistance(max(outer_distance, -inner_distance));
        }
        if (paintOp == PAINT_OP_BORDER && outlineStyle == OUTLINE_DASHED)
            alpha *= dashedOutlineCoverage(local_coord, size, shapeRadius, shapeBorderWidth);
        if (topBorderGap.y > topBorderGap.x) {
            float edge_aa = max(fwidth(local_coord.x), 1.0e-4);
            float inside_gap = smoothstep(topBorderGap.x - edge_aa, topBorderGap.x + edge_aa, local_coord.x)
                * (1.0 - smoothstep(topBorderGap.y - edge_aa, topBorderGap.y + edge_aa, local_coord.x));
            float top_border = step(size.y - widths.x - max(fwidth(outer_distance), 1.0e-4), local_coord.y);
            alpha *= 1.0 - inside_gap * top_border;
        }
    }

    if (paintOp == PAINT_OP_GRADIENT || paintOp == PAINT_OP_GRADIENT_BORDER) {
        vec4 color = filteredGradientColor(local_coord);
        frag_color = vec4(color.rgb, color.a * alpha);
        return;
    }

    frag_color = vec4(shapeColor.rgb, shapeColor.a * alpha);
}

#else
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
#endif
