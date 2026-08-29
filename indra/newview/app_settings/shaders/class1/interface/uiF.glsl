/**
 * Copyright (C) 2007 Linden Research, Inc.
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

// Transitional source variants share one UI shader pair while the viewer
// migrates from the legacy texture/font contract to retained painting. Once
// every UI caller uses retained painting, remove the legacy #else branch and
// make the retained path the default fragment program.

// The text shadow uniform is shared by the legacy font path and captured
// buffer replay; retained paint operations simply leave it unused.
uniform int textShadowMode = 0; // 0 = passthrough, 1 = drop, 2 = soft
uniform vec4 clipCoverageRect;
uniform int clipCoverageEnabled;

float clipCoverage() {
    if (clipCoverageEnabled == 0) return 1.0;
    vec2 size = max(clipCoverageRect.zw, vec2(0.0));
    vec2 point = gl_FragCoord.xy - clipCoverageRect.xy;
    vec2 q = abs(point - size * 0.5) - size * 0.5;
    float signedDistance = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0);
    float aa = max(fwidth(signedDistance), 1.0e-4);
    return 1.0 - smoothstep(-aa * 0.5, aa * 0.5, signedDistance);
}

vec4 applyClipCoverage(vec4 color) {
    color.a *= clipCoverage();
    return color;
}

#ifdef PAINT_SHADER
out vec4 fragColor;

uniform int paintOp;
uniform vec4 shapeRect;
uniform vec4 shapeRadiusX;
uniform vec4 shapeRadiusY;
uniform vec4 innerRadiusX;
uniform vec4 innerRadiusY;
uniform vec4 scrollbarClipRect;
uniform vec4 scrollbarClipRadiusX;
uniform vec4 scrollbarClipRadiusY;
uniform int scrollbarClipEnabled;
uniform float shapeBorderWidth;
uniform vec4 shapeColor;
uniform vec2 shapeOffset;
uniform int arrowDirection;
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
uniform vec4 effectMaskRadiusX;
uniform vec4 effectMaskRadiusY;
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
const int kPaintOpArrow = 9;
const int kGradientLinear = 0;
const int kGradientRadial = 1;
const int kGradientConic = 2;

const int kOutlineSolid = 0;
const int kOutlineDashed = 1;

in vec4 vertexColor;
in vec2 shapeCoord;

vec2 roundedRectCornerRadii(vec2 p, vec2 size, vec4 radiusX, vec4 radiusY) {
    if (p.x < radiusX.x && p.y > size.y - radiusY.x) return vec2(radiusX.x, radiusY.x);
    if (p.x > size.x - radiusX.y && p.y > size.y - radiusY.y) return vec2(radiusX.y, radiusY.y);
    if (p.x > size.x - radiusX.z && p.y < radiusY.z) return vec2(radiusX.z, radiusY.z);
    if (p.x < radiusX.w && p.y < radiusY.w) return vec2(radiusX.w, radiusY.w);
    return vec2(0.0);
}

float roundedRectDistance(vec2 p, vec2 size, vec4 radiusX, vec4 radiusY) {
    vec2 radius = max(roundedRectCornerRadii(p, size, radiusX, radiusY), vec2(0.0));
    vec2 q = abs(p - size * 0.5) - size * 0.5 + radius;
    if (radius.x <= 0.0 || radius.y <= 0.0) {
        vec2 boxQ = abs(p - size * 0.5) - size * 0.5;
        return length(max(boxQ, vec2(0.0))) + min(max(boxQ.x, boxQ.y), 0.0);
    }
    if (q.x > 0.0 && q.y > 0.0) return (length(q / radius) - 1.0) * min(radius.x, radius.y);
    if (q.x > 0.0) return q.x - radius.x;
    if (q.y > 0.0) return q.y - radius.y;
    return max(q.x, q.y);
}

float coverageFromDistance(float signedDistance) {
    float aa = max(fwidth(signedDistance), 1.0e-4);
    return 1.0 - smoothstep(-aa * 0.5, aa * 0.5, signedDistance);
}

float quarterEllipseArc(vec2 radius) {
    if (radius.x <= 0.0 || radius.y <= 0.0) return 0.0;
    float a = max(radius.x, radius.y);
    float b = min(radius.x, radius.y);
    return 0.25 * 3.14159265359 * (3.0 * (a + b) - sqrt(max((3.0 * a + b) * (a + 3.0 * b), 0.0)));
}

float roundedRectPerimeter(vec2 size, vec4 radiusX, vec4 radiusY) {
    vec2 topLeft = vec2(radiusX.x, radiusY.x);
    vec2 topRight = vec2(radiusX.y, radiusY.y);
    vec2 bottomRight = vec2(radiusX.z, radiusY.z);
    vec2 bottomLeft = vec2(radiusX.w, radiusY.w);
    float horizontal = max(size.x - topLeft.x - topRight.x, 0.0) + max(size.x - bottomLeft.x - bottomRight.x, 0.0);
    float vertical = max(size.y - topLeft.y - bottomLeft.y, 0.0) + max(size.y - topRight.y - bottomRight.y, 0.0);
    return horizontal
        + vertical
        + quarterEllipseArc(topLeft)
        + quarterEllipseArc(topRight)
        + quarterEllipseArc(bottomRight)
        + quarterEllipseArc(bottomLeft);
}

float roundedRectPerimeterCoordinate(vec2 p, vec2 size, vec4 radiusX, vec4 radiusY) {
    const float kHalfPi = 1.57079632679;
    const float kPi = 3.14159265359;
    const float kTwoPi = 6.28318530718;
    vec2 topLeft = vec2(radiusX.x, radiusY.x);
    vec2 topRight = vec2(radiusX.y, radiusY.y);
    vec2 bottomRight = vec2(radiusX.z, radiusY.z);
    vec2 bottomLeft = vec2(radiusX.w, radiusY.w);
    float bottom = max(size.x - bottomLeft.x - bottomRight.x, 0.0);
    float right = max(size.y - bottomRight.y - topRight.y, 0.0);
    float top = max(size.x - topLeft.x - topRight.x, 0.0);
    float left = max(size.y - bottomLeft.y - topLeft.y, 0.0);
    float bottomRightArc = quarterEllipseArc(bottomRight);
    float topRightArc = quarterEllipseArc(topRight);
    float topLeftArc = quarterEllipseArc(topLeft);
    float bottomLeftArc = quarterEllipseArc(bottomLeft);
    vec2 point = clamp(p, vec2(0.0), size);

    if (bottomRight.x > 0.0 && bottomRight.y > 0.0 && point.x > size.x - bottomRight.x && point.y < bottomRight.y) {
        vec2 delta = point - vec2(size.x - bottomRight.x, bottomRight.y);
        float angle = atan(delta.y / bottomRight.y, delta.x / bottomRight.x);
        return bottom + bottomRightArc * clamp((angle + kHalfPi) / kHalfPi, 0.0, 1.0);
    }

    if (topRight.x > 0.0 && topRight.y > 0.0 && point.x > size.x - topRight.x && point.y > size.y - topRight.y) {
        vec2 delta = point - vec2(size.x - topRight.x, size.y - topRight.y);
        float angle = atan(delta.y / topRight.y, delta.x / topRight.x);
        return bottom + bottomRightArc + right + topRightArc * clamp(angle / kHalfPi, 0.0, 1.0);
    }

    if (topLeft.x > 0.0 && topLeft.y > 0.0 && point.x < topLeft.x && point.y > size.y - topLeft.y) {
        vec2 delta = point - vec2(topLeft.x, size.y - topLeft.y);
        float angle = atan(delta.y / topLeft.y, delta.x / topLeft.x);
        return bottom + bottomRightArc + right + topRightArc + top + topLeftArc * clamp((angle - kHalfPi) / kHalfPi, 0.0, 1.0);
    }

    if (bottomLeft.x > 0.0 && bottomLeft.y > 0.0 && point.x < bottomLeft.x && point.y < bottomLeft.y) {
        vec2 delta = point - vec2(bottomLeft.x, bottomLeft.y);
        float angle = atan(delta.y / bottomLeft.y, delta.x / bottomLeft.x);
        if (angle < 0.0) angle += kTwoPi;
        return bottom + bottomRightArc + right + topRightArc + top + topLeftArc + left + bottomLeftArc * clamp((angle - kPi) / kHalfPi, 0.0, 1.0);
    }

    float bottomDistance = abs(point.y);
    float rightDistance = abs(size.x - point.x);
    float topDistance = abs(size.y - point.y);
    float leftDistance = abs(point.x);
    float nearest = min(min(bottomDistance, rightDistance), min(topDistance, leftDistance));
    if (nearest == bottomDistance) return clamp(point.x - bottomLeft.x, 0.0, bottom);
    if (nearest == rightDistance) return bottom + bottomRightArc + clamp(point.y - bottomRight.y, 0.0, right);
    if (nearest == topDistance) return bottom + bottomRightArc + right + topRightArc + clamp(size.x - topRight.x - point.x, 0.0, top);
    return bottom + bottomRightArc + right + topRightArc + top + topLeftArc + clamp(size.y - topLeft.y - point.y, 0.0, left);
}

float dashedOutlineCoverage(vec2 localCoord, vec2 size, vec4 radiusX, vec4 radiusY, float width) {
    float centerOffset = width * 0.5;
    vec2 centerSize = max(size - vec2(width), vec2(0.0));
    vec4 centerRadiusX = max(radiusX - vec4(centerOffset), vec4(0.0));
    vec4 centerRadiusY = max(radiusY - vec4(centerOffset), vec4(0.0));
    float perimeter = roundedRectPerimeter(centerSize, centerRadiusX, centerRadiusY);
    if (perimeter <= 1.0e-4) return 1.0;
    float preferredPeriod = max(width * 5.0, 1.0);
    float dashCount = max(floor(perimeter / preferredPeriod + 0.5), 1.0);
    float period = perimeter / dashCount;
    float dashLength = period * 0.6;
    float coordinate = roundedRectPerimeterCoordinate(localCoord - vec2(centerOffset), centerSize, centerRadiusX, centerRadiusY);
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
        float distance = roundedRectDistance(point - effectMaskRect.xy, effectMaskRect.zw, effectMaskRadiusX, effectMaskRadiusY);
        float mask = coverageFromDistance(distance);
        return vec4(color.rgb, mask);
    }

    color.rgb = color.a > 1.0e-6 ? color.rgb / color.a : vec3(0.0);
    return color;
}

float triangleEdge(vec2 a, vec2 b, vec2 p) {
    return (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
}

float roundedTriangleCornerDistance(vec2 p, vec2 vertex, vec2 incomingNormal, vec2 outgoingNormal, float radius) {
    vec2 bisector = normalize(incomingNormal + outgoingNormal);
    float sinHalfAngle = max(dot(outgoingNormal, bisector), 1.0e-4);
    vec2 center = vertex + bisector * (radius / sinHalfAngle);
    return radius - length(p - center);
}

float roundedTriangleCoverage(vec2 p, vec2 a, vec2 b, vec2 c, float radius) {
    float winding = triangleEdge(a, b, c) >= 0.0 ? 1.0 : -1.0;
    vec2 edgeAB = b - a;
    vec2 edgeBC = c - b;
    vec2 edgeCA = a - c;
    vec2 normalAB = winding * vec2(-edgeAB.y, edgeAB.x) / max(length(edgeAB), 1.0e-4);
    vec2 normalBC = winding * vec2(-edgeBC.y, edgeBC.x) / max(length(edgeBC), 1.0e-4);
    vec2 normalCA = winding * vec2(-edgeCA.y, edgeCA.x) / max(length(edgeCA), 1.0e-4);
    float edgeA = dot(p - a, normalAB);
    float edgeB = dot(p - b, normalBC);
    float edgeC = dot(p - c, normalCA);
    float edgeDistance = min(min(edgeA, edgeB), edgeC);
    float cornerAIncoming = dot(p - a, normalCA);
    float cornerAOutgoing = dot(p - a, normalAB);
    float cornerBIncoming = dot(p - b, normalAB);
    float cornerBOutgoing = dot(p - b, normalBC);
    float cornerCIncoming = dot(p - c, normalBC);
    float cornerCOutgoing = dot(p - c, normalCA);
    if (cornerAIncoming < radius && cornerAOutgoing < radius)
        edgeDistance = roundedTriangleCornerDistance(p, a, normalCA, normalAB, radius);
    else if (cornerBIncoming < radius && cornerBOutgoing < radius)
        edgeDistance = roundedTriangleCornerDistance(p, b, normalAB, normalBC, radius);
    else if (cornerCIncoming < radius && cornerCOutgoing < radius)
        edgeDistance = roundedTriangleCornerDistance(p, c, normalBC, normalCA, radius);
    float aa = max(fwidth(edgeDistance), 1.0e-4);
    return smoothstep(-aa * 0.5, aa * 0.5, edgeDistance);
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
        fragColor = applyClipCoverage(vertexColor);
        return;
    }

    if (paintOp == kPaintOpBlur) {
        fragColor = applyClipCoverage(blurredEffectColor(shapeCoord));
        return;
    }

    if (paintOp == kPaintOpComposite) {
        fragColor = applyClipCoverage(compositedEffectColor(shapeCoord));
        return;
    }

    if (paintOp == kPaintOpArrow) {
        vec2 size = max(shapeRect.zw, vec2(0.0));
        vec2 center = size * 0.5;
        float arrowLength = min(size.x, size.y) * 0.30;
        float arrowWidth = min(size.x, size.y) * 0.35;
        float arrowRadius = min(size.x, size.y) * 0.10;
        vec2 a;
        vec2 b;
        vec2 c;
        if (arrowDirection == 0) {
            a = center + vec2(arrowLength, -arrowWidth);
            b = center + vec2(arrowLength, arrowWidth);
            c = center + vec2(-arrowLength, 0.0);
        } else if (arrowDirection == 1) {
            a = center + vec2(-arrowLength, -arrowWidth);
            b = center + vec2(-arrowLength, arrowWidth);
            c = center + vec2(arrowLength, 0.0);
        } else if (arrowDirection == 2) {
            a = center + vec2(-arrowWidth, arrowLength);
            b = center + vec2(arrowWidth, arrowLength);
            c = center + vec2(0.0, -arrowLength);
        } else {
            a = center + vec2(-arrowWidth, -arrowLength);
            b = center + vec2(arrowWidth, -arrowLength);
            c = center + vec2(0.0, arrowLength);
        }
        float alpha = roundedTriangleCoverage(shapeCoord, a, b, c, arrowRadius);
        if (scrollbarClipEnabled != 0) {
            vec2 clipCoord = shapeCoord - scrollbarClipRect.xy;
            float clipDistance = roundedRectDistance(clipCoord, scrollbarClipRect.zw, scrollbarClipRadiusX, scrollbarClipRadiusY);
            alpha *= coverageFromDistance(clipDistance);
        }
        fragColor = applyClipCoverage(vec4(shapeColor.rgb, shapeColor.a * alpha));
        return;
    }

    vec2 size = max(shapeRect.zw, vec2(0.0));
    vec2 localCoord = shapeCoord - shapeOffset;
    float outerDistance = roundedRectDistance(localCoord, size, shapeRadiusX, shapeRadiusY);
    float alpha = coverageFromDistance(outerDistance);
    if (scrollbarClipEnabled != 0) {
        vec2 clipCoord = localCoord - scrollbarClipRect.xy;
        float clipDistance = roundedRectDistance(clipCoord, scrollbarClipRect.zw, scrollbarClipRadiusX, scrollbarClipRadiusY);
        alpha *= coverageFromDistance(clipDistance);
    }

    if (paintOp == kPaintOpOuterShadow) {
        float softness = max(shadowBlur, fwidth(outerDistance));
        alpha = 1.0 - smoothstep(-softness, softness, outerDistance);
        fragColor = applyClipCoverage(vec4(shapeColor.rgb, shapeColor.a * alpha));
        return;
    }

    if (paintOp == kPaintOpInsetShadow) {
        vec2 holeSize = max(size - vec2(shadowSpread * 2.0), vec2(0.0));
        vec2 holeCoord = localCoord - vec2(shadowSpread) - shadowOffset;
        float holeDistance = roundedRectDistance(holeCoord, holeSize, innerRadiusX, innerRadiusY);
        float softness = max(shadowBlur, fwidth(holeDistance));
        alpha *= smoothstep(-softness, softness, holeDistance);
        fragColor = applyClipCoverage(vec4(shapeColor.rgb, shapeColor.a * alpha));
        return;
    }

    if (paintOp == kPaintOpBorder || paintOp == kPaintOpGradientBorder) {
        vec4 widths = paintOp == kPaintOpBorder ? vec4(shapeBorderWidth) : borderWidths;
        widths = max(widths, vec4(0.0));
        vec2 innerSize = max(size - vec2(widths.w + widths.y, widths.z + widths.x), vec2(0.0));
        vec2 innerCoord = localCoord - vec2(widths.w, widths.z);
        if (innerSize.x > 0.0 && innerSize.y > 0.0) {
            float innerDistance = roundedRectDistance(innerCoord, innerSize, innerRadiusX, innerRadiusY);
            alpha = coverageFromDistance(max(outerDistance, -innerDistance));
        }
        if (paintOp == kPaintOpBorder && outlineStyle == kOutlineDashed)
            alpha *= dashedOutlineCoverage(localCoord, size, shapeRadiusX, shapeRadiusY, shapeBorderWidth);
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
        fragColor = applyClipCoverage(vec4(color.rgb, color.a * alpha));
        return;
    }

    fragColor = applyClipCoverage(vec4(shapeColor.rgb, shapeColor.a * alpha));
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
                fragColor = applyClipCoverage(vec4(vertexColor.rgb, premul.a * vertexColor.a));
                return;
            }
            fragColor = applyClipCoverage(vec4(premul.rgb / max(premul.a, 1e-5), premul.a * vertexColor.a));
            return;
        }

        float coverage = hb_gpu_draw(vary_texcoord0, vary_glyphLoc);
        float ppem = hb_gpu_ppem(vary_texcoord0, vary_glyphLoc);
        coverage = alchemy_font_refine_coverage(coverage, vertexColor.rgb, ppem);
        fragColor = applyClipCoverage(vec4(vertexColor.rgb, vertexColor.a * coverage));
        return;
    }
#endif
    if (textShadowMode == 0) {
        fragColor = applyClipCoverage(vertexColor * texture(diffuseMap, vary_texcoord0.xy));
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
    fragColor = applyClipCoverage(vec4(vertexColor.rgb, 1.0 - p));
}
#endif
