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

// The text shadow uniform is shared by the legacy font path and captured
// buffer replay; retained paint operations simply leave it unused.
uniform int textShadowMode = 0; // 0 = passthrough, 1 = drop, 2 = soft

#ifdef PAINT_SHADER
out vec4 fragColor;

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

const int kPaintOpDirect = 0;
const int kPaintOpFill = 1;
const int kPaintOpBorder = 2;
const int kPaintOpGradient = 3;
const int kPaintOpOuterShadow = 4;
const int kPaintOpInsetShadow = 5;
const int kPaintOpGradientBorder = 6;
const int kPaintOpBlur = 7;
const int kPaintOpComposite = 8;
const int kGradientLinear = 0;
const int kGradientRadial = 1;
const int kGradientConic = 2;

const int kOutlineSolid = 0;
const int kOutlineDashed = 1;

in vec4 vertexColor;
in vec2 shapeCoord;

float roundedRectDistance(vec2 p, vec2 size, float radius) {
    float r = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    vec2 halfSize = size * 0.5;
    vec2 q = abs(p - halfSize) - (halfSize - vec2(r));
    return length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

float coverageFromDistance(float signedDistance) {
    float aa = max(fwidth(signedDistance), 1.0e-4);
    return 1.0 - smoothstep(-aa * 0.5, aa * 0.5, signedDistance);
}

float roundedRectPerimeterCoordinate(vec2 p, vec2 size, float radius) {
    const float kHalfPi = 1.57079632679;
    const float kPi = 3.14159265359;
    const float kTwoPi = 6.28318530718;
    float r = clamp(radius, 0.0, min(size.x, size.y) * 0.5);
    float horizontal = max(size.x - r * 2.0, 0.0);
    float vertical = max(size.y - r * 2.0, 0.0);
    vec2 point = clamp(p, vec2(0.0), size);

    if (r > 0.0 && point.x > size.x - r && point.y < r) {
        float angle = atan(point.y - r, point.x - (size.x - r));
        return horizontal + r * (angle + kHalfPi);
    }

    if (r > 0.0 && point.x > size.x - r && point.y > size.y - r) {
        float angle = atan(point.y - (size.y - r), point.x - (size.x - r));
        return horizontal + r * kHalfPi + vertical + r * angle;
    }

    if (r > 0.0 && point.x < r && point.y > size.y - r) {
        float angle = atan(point.y - (size.y - r), point.x - r);
        return horizontal * 2.0 + r * 2.0 * kHalfPi + vertical + r * (angle - kHalfPi);
    }

    if (r > 0.0 && point.x < r && point.y < r) {
        float angle = atan(point.y - r, point.x - r);
        if (angle < 0.0) angle += kTwoPi;
        return horizontal * 2.0 + r * 3.0 * kHalfPi + vertical * 2.0 + r * (angle - kPi);
    }

    float bottomDistance = abs(point.y);
    float rightDistance = abs(size.x - point.x);
    float topDistance = abs(size.y - point.y);
    float leftDistance = abs(point.x);
    float nearest = min(min(bottomDistance, rightDistance), min(topDistance, leftDistance));
    if (nearest == bottomDistance) return clamp(point.x - r, 0.0, horizontal);
    if (nearest == rightDistance) return horizontal + r * kHalfPi + clamp(point.y - r, 0.0, vertical);
    if (nearest == topDistance) return horizontal + r * 2.0 * kHalfPi + vertical + clamp(size.x - r - point.x, 0.0, horizontal);
    return horizontal * 2.0 + r * 3.0 * kHalfPi + vertical + clamp(size.y - r - point.y, 0.0, vertical);
}

float dashedOutlineCoverage(vec2 localCoord, vec2 size, float radius, float width) {
    float centerOffset = width * 0.5;
    vec2 centerSize = max(size - vec2(width), vec2(0.0));
    float centerRadius = max(radius - centerOffset, 0.0);
    float horizontal = max(centerSize.x - centerRadius * 2.0, 0.0);
    float vertical = max(centerSize.y - centerRadius * 2.0, 0.0);
    float perimeter = horizontal * 2.0 + vertical * 2.0 + centerRadius * 6.28318530718;
    if (perimeter <= 1.0e-4) return 1.0;
    float preferredPeriod = max(width * 5.0, 1.0);
    float dashCount = max(floor(perimeter / preferredPeriod + 0.5), 1.0);
    float period = perimeter / dashCount;
    float dashLength = period * 0.6;
    float coordinate = roundedRectPerimeterCoordinate(localCoord - vec2(centerOffset), centerSize, centerRadius);
    float phase = mod(coordinate, period);
    float dashDistance = abs(phase - dashLength * 0.5) - dashLength * 0.5;
    float aa = max(fwidth(coordinate), 1.0e-4);
    return 1.0 - smoothstep(-aa * 0.5, aa * 0.5, dashDistance);
}

vec4 blurredEffectColor(vec2 textureCoord) {
    vec2 pixelCoord = textureCoord * effectTextureSize;
    vec2 gradientAxis = effectGradientEnd - effectGradientStart;
    float amount = clamp(dot(pixelCoord - effectGradientStart, gradientAxis) / max(dot(gradientAxis, gradientAxis), 1.0e-6), 0.0, 1.0);
    float radius = mix(effectBlurRadii.x, effectBlurRadii.y, amount);
    if (radius <= 1.0e-4) return texture(diffuseMap, textureCoord);

    const int maxSamplesPerSide = 32;
    float samplesPerSide = min(ceil(radius), float(maxSamplesPerSide));
    float sampleStep = radius / samplesPerSide;
    float sigma = max(radius / 3.0, 0.5);
    float inverseTwoSigmaSquared = 0.5 / (sigma * sigma);
    vec2 texel = effectBlurAxis / max(effectTextureSize, vec2(1.0));
    vec4 color = vec4(0.0);
    float totalWeight = 0.0;
    for (int sampleIndex = -maxSamplesPerSide; sampleIndex <= maxSamplesPerSide; ++sampleIndex) {
        float sampleOffset = float(sampleIndex) * sampleStep;
        if (abs(sampleOffset) > radius + 1.0e-4) continue;
        float weight = exp(-sampleOffset * sampleOffset * inverseTwoSigmaSquared);
        color += texture(diffuseMap, textureCoord + texel * sampleOffset) * weight;
        totalWeight += weight;
    }
    return color / max(totalWeight, 1.0e-6);
}

vec4 compositedEffectColor(vec2 textureCoord) {
    vec4 color = texture(diffuseMap, textureCoord);
    if (effectRoundedMask != 0) {
        vec2 point = effectCaptureRect.xy + textureCoord * effectCaptureRect.zw;
        float distance = roundedRectDistance(point - effectMaskRect.xy, effectMaskRect.zw, effectMaskRadius);
        float mask = coverageFromDistance(distance);
        return vec4(color.rgb, mask);
    }

    color.rgb = color.a > 1.0e-6 ? color.rgb / color.a : vec3(0.0);
    return color;
}

float gradientAmount(vec2 localCoord) {
    float amount = 0.0;
    if (gradientKind == kGradientLinear) {
        vec2 axis = gradientEnd - gradientStart;
        amount = dot(localCoord - gradientStart, axis) / max(dot(axis, axis), 1.0e-6);
    } else if (gradientKind == kGradientRadial) amount = length((localCoord - gradientCenter) / max(gradientRadius, vec2(1.0e-4)));
    else {
        const float tau = 6.28318530718;
        vec2 delta = localCoord - gradientCenter;
        amount = atan(delta.x, delta.y) / tau - gradientAngle / 360.0;
    }

    return amount;
}

float gradientPixelWidth(vec2 localCoord, float amount) {
    if (gradientKind != kGradientConic) return fwidth(amount);

    const float tau = 6.28318530718;
    vec2 delta = localCoord - gradientCenter;
    vec2 deltaDx = dFdx(delta);
    vec2 deltaDy = dFdy(delta);
    float radiusSquared = dot(delta, delta);
    float footprintSquared = max(dot(deltaDx, deltaDx), dot(deltaDy, deltaDy));
    if (radiusSquared <= footprintSquared * 0.25) return 1.0;
    radiusSquared = max(radiusSquared, 1.0e-6);
    float turnDx = (delta.y * deltaDx.x - delta.x * deltaDx.y) / (tau * radiusSquared);
    float turnDy = (delta.y * deltaDy.x - delta.x * deltaDy.y) / (tau * radiusSquared);
    return abs(turnDx) + abs(turnDy);
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

vec4 underlyingGradientIntegral(float amount, vec4 repeatingTotal) {
    if (gradientRepeating == 0) return clampedGradientIntegral(amount);

    float first = gradientStops[0];
    float period = gradientStops[gradientStopCount - 1] - first;
    float cycles = floor((amount - first) / period);
    float wrapped = amount - first - cycles * period;
    return cycles * repeatingTotal + gradientIntervalIntegral(first + wrapped);
}

vec4 filteredGradientColor(vec2 localCoord) {
    float amount = gradientAmount(localCoord);
    float pixelWidth = max(gradientPixelWidth(localCoord, amount), 1.0e-6);
    float start = amount - pixelWidth * 0.5;
    float end = amount + pixelWidth * 0.5;
    if (gradientKind == kGradientRadial) start = max(start, 0.0);

    vec4 repeatingTotal = vec4(0.0);
    if (gradientRepeating != 0) repeatingTotal = gradientIntervalIntegral(gradientStops[gradientStopCount - 1]);

    vec4 startIntegral;
    vec4 endIntegral;
    if (gradientKind == kGradientConic) {
        vec4 origin = underlyingGradientIntegral(0.0, repeatingTotal);
        vec4 turnTotal = underlyingGradientIntegral(1.0, repeatingTotal) - origin;
        float startTurn = floor(start);
        float endTurn = floor(end);
        startIntegral = startTurn * turnTotal + underlyingGradientIntegral(start - startTurn, repeatingTotal) - origin;
        endIntegral = endTurn * turnTotal + underlyingGradientIntegral(end - endTurn, repeatingTotal) - origin;
    } else {
        startIntegral = underlyingGradientIntegral(start, repeatingTotal);
        endIntegral = underlyingGradientIntegral(end, repeatingTotal);
    }
    return (endIntegral - startIntegral) / max(end - start, 1.0e-6);
}

void main() {
    if (paintOp == kPaintOpDirect) {
        fragColor = vertexColor;
        return;
    }

    if (paintOp == kPaintOpBlur) {
        fragColor = blurredEffectColor(shapeCoord);
        return;
    }

    if (paintOp == kPaintOpComposite) {
        fragColor = compositedEffectColor(shapeCoord);
        return;
    }

    vec2 size = max(shapeRect.zw, vec2(0.0));
    vec2 localCoord = shapeCoord - shapeOffset;
    float outerDistance = roundedRectDistance(localCoord, size, shapeRadius);
    float alpha = coverageFromDistance(outerDistance);

    if (paintOp == kPaintOpOuterShadow) {
        float softness = max(shadowBlur, fwidth(outerDistance));
        alpha = 1.0 - smoothstep(-softness, softness, outerDistance);
        fragColor = vec4(shapeColor.rgb, shapeColor.a * alpha);
        return;
    }

    if (paintOp == kPaintOpInsetShadow) {
        vec2 holeSize = max(size - vec2(shadowSpread * 2.0), vec2(0.0));
        vec2 holeCoord = localCoord - vec2(shadowSpread) - shadowOffset;
        float holeRadius = max(shapeRadius - shadowSpread, 0.0);
        float holeDistance = roundedRectDistance(holeCoord, holeSize, holeRadius);
        float softness = max(shadowBlur, fwidth(holeDistance));
        alpha *= smoothstep(-softness, softness, holeDistance);
        fragColor = vec4(shapeColor.rgb, shapeColor.a * alpha);
        return;
    }

    if (paintOp == kPaintOpBorder || paintOp == kPaintOpGradientBorder) {
        vec4 widths = paintOp == kPaintOpBorder ? vec4(shapeBorderWidth) : borderWidths;
        widths = max(widths, vec4(0.0));
        vec2 innerSize = max(size - vec2(widths.w + widths.y, widths.z + widths.x), vec2(0.0));
        vec2 innerCoord = localCoord - vec2(widths.w, widths.z);
        float innerRadius = max(shapeRadius - max(max(widths.x, widths.y), max(widths.z, widths.w)), 0.0);
        if (innerSize.x > 0.0 && innerSize.y > 0.0) {
            float innerDistance = roundedRectDistance(innerCoord, innerSize, innerRadius);
            alpha = coverageFromDistance(max(outerDistance, -innerDistance));
        }
        if (paintOp == kPaintOpBorder && outlineStyle == kOutlineDashed)
            alpha *= dashedOutlineCoverage(localCoord, size, shapeRadius, shapeBorderWidth);
        if (topBorderGap.y > topBorderGap.x) {
            float edgeAA = max(fwidth(localCoord.x), 1.0e-4);
            float insideGap = smoothstep(topBorderGap.x - edgeAA, topBorderGap.x + edgeAA, localCoord.x)
                * (1.0 - smoothstep(topBorderGap.y - edgeAA, topBorderGap.y + edgeAA, localCoord.x));
            float topBorder = step(size.y - widths.x - max(fwidth(outerDistance), 1.0e-4), localCoord.y);
            alpha *= 1.0 - insideGap * topBorder;
        }
    }

    if (paintOp == kPaintOpGradient || paintOp == kPaintOpGradientBorder) {
        vec4 color = filteredGradientColor(localCoord);
        fragColor = vec4(color.rgb, color.a * alpha);
        return;
    }

    fragColor = vec4(shapeColor.rgb, shapeColor.a * alpha);
}

#else
out vec4 fragColor;

uniform sampler2D diffuseMap;

in vec2 vary_texcoord0;
in vec4 vertexColor;

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
            vec4 premul = hb_gpu_paint(vary_texcoord0, vary_glyphLoc & 0x3FFFFFFFu, vec4(vertexColor.rgb, 1.0), coverage);
            if ((vary_glyphLoc & 0x40000000u) != 0u) {
                fragColor = vec4(vertexColor.rgb, premul.a * vertexColor.a);
                return;
            }
            fragColor = vec4(premul.rgb / max(premul.a, 1e-5), premul.a * vertexColor.a);
            return;
        }

        float coverage = hb_gpu_draw(vary_texcoord0, vary_glyphLoc);
        float ppem = hb_gpu_ppem(vary_texcoord0, vary_glyphLoc);
        coverage = alchemy_font_refine_coverage(coverage, vertexColor.rgb, ppem);
        fragColor = vec4(vertexColor.rgb, vertexColor.a * coverage);
        return;
    }
#endif
    if (textShadowMode == 0) {
        fragColor = vertexColor * texture(diffuseMap, vary_texcoord0.xy);
        return;
    }

    vec2 atlasTexelSize = 1.0 / vec2(textureSize(diffuseMap, 0));
    float vertexColorAlpha = vertexColor.a;
    float p;
    if (textShadowMode == 1) {
        p = 1.0 - vertexColorAlpha * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(-1.0, 1.0));
    } else {
        p = (1.0 - vertexColorAlpha * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(1.0, 1.0)));
        p *= (1.0 - vertexColorAlpha * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(-1.0, 1.0)));
        p *= (1.0 - vertexColorAlpha * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(-1.0, -1.0)));
        p *= (1.0 - vertexColorAlpha * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(1.0, -1.0)));
        p *= (1.0 - vertexColorAlpha * sampleAtlasAlpha(vary_texcoord0 + atlasTexelSize * vec2(0.0, 2.0)));
    }
    fragColor = vec4(vertexColor.rgb, 1.0 - p);
}
#endif
