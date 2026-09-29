/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <Core/CSSRules.h>
#include <Core/Color.h>
#include <Core/Document.h>
#include <Core/HTMLButtonElement.h>
#include <Core/HTMLFloaterElement.h>
#include <Core/HTMLInputElement.h>
#include <Core/HTMLLabelElement.h>
#include <Core/HTMLPanelElement.h>
#include <Core/ResourceElementDefinition.h>
#include <Core/StyleSheet.h>
#include <Core/Text.h>
#include <Core/TextMeasurer.h>
#include <cstddef>
#include <cstdint>
#include <gtest/gtest.h>
#include <string>
#include <utility>
#include <variant>
#include <vector>

namespace {
using Core::Color;
using Core::ResourceLayer;
using Core::CSS::StyleLayer;
using Core::CSS::StyleOrigin;
using Core::CSS::StyleSheet;
using Core::Style::BackgroundAttachment;
using Core::Style::BackgroundBox;
using Core::Style::BackgroundRepeat;
using Core::Style::BackgroundSizeType;
using Core::Style::BlurFilter;
using Core::Style::ComputedStyle;
using Core::Style::CursorStyle;
using Core::Style::Gradient;
using Core::Style::GradientKind;
using Core::Style::Image;
using Core::Style::LinearBlurFilter;
using Core::Style::MaskComposite;
using Core::Style::MaskLayer;
using Core::Style::MaskMode;
using Core::Style::MaskType;
using Core::Style::RadialGradientShape;
} // namespace

TEST(StyleSheet, ParsesFilter) {
    constexpr char kBoxEffectStyles[] = "panel { background-image: linear-gradient(to/**/right, #ff0000ff, "
                                        "rgb(0, 255, 0, 50%) 75%, #0000ffff); "
                                        "box-shadow: 1px 2px #11223344, 3px 4px 5px 6px "
                                        "rgb(10, 20, 30, 40%) inset; outline-offset: 3px; "
                                        "outline: light-dark(#abcdef88, #12345688) solid 2px; } panel.light { color-scheme: light; } "
                                        "label { outline: 1px dashed #ffffffff; }";
    constexpr char kFilterStyles[] = "panel { backdrop-filter: linear-blur(to bottom, 0px 25%, 16px 75%); "
                                     "filter: linear-blur(to right, 2px 25%, 6px 75%) blur(4px); } "
                                     "button { filter: linear-blur(8px); } label { --blur-radius: 4px; "
                                     "filter: blur(var(--blur-radius)); backdrop-filter: none; }";
    constexpr char kGradientStyles[] = "panel { background-image: radial-gradient(circle at 25% 75%, "
                                       "#ffffffff, #00000000 80%); border-width: 3px; } button { border-width: 2px; border-style: solid; } "
                                       "input { background-image: repeating-radial-gradient(ellipse at center, "
                                       "#ffffffff 0%, #000000ff 25%); } floater { background-image: "
                                       "repeating-conic-gradient(from .25turn, #ffffffff 0deg 30deg, "
                                       "#000000ff 60deg); } panel.solid { background-color: #112233ff; "
                                       "border-color: #445566ff; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBoxEffectStyles).ok());
    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    const Gradient* boxGradient = style.backgroundLayers.front().image.gradient();
    ASSERT_NE(boxGradient, nullptr);
    EXPECT_EQ(boxGradient->kind, GradientKind::Linear);
    EXPECT_EQ(boxGradient->angleDegrees, 90.f);
    EXPECT_EQ(boxGradient->stops.size(), std::size_t(3));
    EXPECT_NEAR(boxGradient->stops[1].position, .75f, 1.0e-4f);
    EXPECT_NEAR(boxGradient->stops[1].color.resolvedColor().a, .5f, 1.0e-4f);
    EXPECT_EQ(style.boxShadow().size(), std::size_t(2));
    EXPECT_EQ(style.boxShadow()[0].blur, 0.f);
    EXPECT_FALSE(style.boxShadow()[0].inset);
    EXPECT_EQ(style.boxShadow()[1].spread, 6.f);
    EXPECT_TRUE(style.boxShadow()[1].inset);
    EXPECT_EQ(style.outline().width, 2.f);
    EXPECT_EQ(style.outline().offset.pixels, 3.f);
    EXPECT_NEAR(style.outline().color.resolvedColor().r, 18.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(stylesheet.resolve("panel", "", {"light"}).outline().color.resolvedColor().r, 171.f / 255.f, 1.0e-4f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).outline().offset.pixels, 0.f);
    EXPECT_EQ(stylesheet.resolve("label", "", {}).outline().style, Core::Style::OutlineStyle::Dashed);

    ASSERT_TRUE(stylesheet.loadRadia(kFilterStyles).ok());
    const ComputedStyle filters = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(filters.backdropFilter().operations.size(), std::size_t(1));
    const LinearBlurFilter* backdrop = std::get_if<LinearBlurFilter>(&filters.backdropFilter().operations.front());
    ASSERT_NE(backdrop, nullptr);
    ASSERT_EQ(backdrop->stops.size(), std::size_t(2));
    EXPECT_EQ(backdrop->stops[0].stdDeviation, 0.f);
    EXPECT_EQ(backdrop->stops[1].stdDeviation, 16.f);
    EXPECT_EQ(backdrop->stops[0].position, .25f);
    EXPECT_EQ(backdrop->stops[1].position, .75f);
    EXPECT_EQ(backdrop->angleDegrees, 180.f);
    ASSERT_EQ(filters.filter.operations.size(), std::size_t(2));
    const LinearBlurFilter* progressive = std::get_if<LinearBlurFilter>(&filters.filter.operations[0]);
    ASSERT_NE(progressive, nullptr);
    ASSERT_EQ(progressive->stops.size(), std::size_t(2));
    EXPECT_EQ(progressive->stops[0].stdDeviation, 2.f);
    EXPECT_EQ(progressive->stops[1].stdDeviation, 6.f);
    EXPECT_EQ(progressive->stops[0].position, .25f);
    EXPECT_EQ(progressive->stops[1].position, .75f);
    EXPECT_EQ(progressive->angleDegrees, 90.f);
    const BlurFilter* uniform = std::get_if<BlurFilter>(&filters.filter.operations[1]);
    ASSERT_NE(uniform, nullptr);
    EXPECT_EQ(uniform->stdDeviation, 4.f);
    const ComputedStyle button = stylesheet.resolve("button", "", {});
    const LinearBlurFilter* uniformLinear = std::get_if<LinearBlurFilter>(&button.filter.operations.front());
    ASSERT_NE(uniformLinear, nullptr);
    ASSERT_EQ(uniformLinear->stops.size(), std::size_t(2));
    EXPECT_EQ(uniformLinear->stops[0].stdDeviation, 8.f);
    EXPECT_EQ(uniformLinear->stops[1].stdDeviation, 8.f);
    EXPECT_EQ(uniformLinear->stops[0].position, 0.f);
    EXPECT_EQ(uniformLinear->stops[1].position, 1.f);
    const ComputedStyle label = stylesheet.resolve("label", "", {});
    ASSERT_EQ(label.filter.operations.size(), std::size_t(1));
    const BlurFilter* nested = std::get_if<BlurFilter>(&label.filter.operations.front());
    ASSERT_NE(nested, nullptr);
    EXPECT_EQ(nested->stdDeviation, 4.f);
    EXPECT_TRUE(label.backdropFilter().isNone());

    ASSERT_TRUE(stylesheet.loadRadia(kGradientStyles).ok());
    const ComputedStyle radial = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(radial.backgroundLayers.size(), 1U);
    const Gradient* radialGradient = radial.backgroundLayers.front().image.gradient();
    ASSERT_NE(radialGradient, nullptr);
    EXPECT_EQ(radialGradient->kind, GradientKind::Radial);
    EXPECT_EQ(radialGradient->radialShape, RadialGradientShape::Circle);
    EXPECT_NEAR(radialGradient->center.x, .25f, 1.0e-4f);
    EXPECT_NEAR(radialGradient->center.y, .25f, 1.0e-4f);
    EXPECT_EQ(radial.borderWidth().top.pixels, 3.f);
    const ComputedStyle repeatingBorder = stylesheet.resolve("button", "", {});
    EXPECT_EQ(repeatingBorder.borderWidth().left.pixels, 2.f);
    const ComputedStyle repeatingRadial = stylesheet.resolve("input", "", {});
    ASSERT_EQ(repeatingRadial.backgroundLayers.size(), 1U);
    const Gradient* repeatingRadialGradient = repeatingRadial.backgroundLayers.front().image.gradient();
    ASSERT_NE(repeatingRadialGradient, nullptr);
    EXPECT_EQ(repeatingRadialGradient->kind, GradientKind::Radial);
    EXPECT_TRUE(repeatingRadialGradient->repeating);
    const ComputedStyle repeatingConic = stylesheet.resolve("floater", "", {});
    ASSERT_EQ(repeatingConic.backgroundLayers.size(), 1U);
    const Gradient* repeatingConicGradient = repeatingConic.backgroundLayers.front().image.gradient();
    ASSERT_NE(repeatingConicGradient, nullptr);
    EXPECT_EQ(repeatingConicGradient->kind, GradientKind::Conic);
    EXPECT_TRUE(repeatingConicGradient->repeating);
    EXPECT_EQ(repeatingConicGradient->angleDegrees, 90.f);
    const ComputedStyle solidOverride = stylesheet.resolve("panel", "", {"solid"});
    ASSERT_EQ(solidOverride.backgroundLayers.size(), 1U);
    ASSERT_NE(solidOverride.backgroundLayers.front().image.gradient(), nullptr);
    EXPECT_NEAR(solidOverride.backgroundColor().resolvedColor().r, 17.f / 255.f, 1.0e-4f);
    EXPECT_NEAR(solidOverride.borderColor().top.resolvedColor().g, 85.f / 255.f, 1.0e-4f);
}

TEST(StyleSheet, ParsesOutOfRangeRadialCenter) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("panel { background-image: radial-gradient(at -25% 125%, #fff, #000); }").ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    const Gradient* gradient = style.backgroundLayers.front().image.gradient();
    ASSERT_NE(gradient, nullptr);
    EXPECT_NEAR(gradient->center.x, -.25f, 1.0e-4f);
    EXPECT_NEAR(gradient->center.y, -.25f, 1.0e-4f);
}

TEST(StyleSheet, ParsesGradientAnglesAndSingleStopGradients) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { background-image: linear-gradient(0, #fff, #000); } "
                                             "button { background-image: linear-gradient(100grad, #fff, #000); "
                                             "filter: linear-blur(100grad, 2px 25%, 6px 75%); } "
                                             "input { background-image: linear-gradient(1e38turn, #fff, #000); } "
                                             "floater { background-image: linear-gradient(red); }");
    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());

    const ComputedStyle zeroAngle = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(zeroAngle.backgroundLayers.size(), 1U);
    ASSERT_NE(zeroAngle.backgroundLayers.front().image.gradient(), nullptr);
    EXPECT_FLOAT_EQ(zeroAngle.backgroundLayers.front().image.gradient()->angleDegrees, 0.f);

    const ComputedStyle gradAngle = stylesheet.resolve("button", "", {});
    ASSERT_EQ(gradAngle.backgroundLayers.size(), 1U);
    ASSERT_NE(gradAngle.backgroundLayers.front().image.gradient(), nullptr);
    EXPECT_FLOAT_EQ(gradAngle.backgroundLayers.front().image.gradient()->angleDegrees, 90.f);
    ASSERT_EQ(gradAngle.filter.operations.size(), 1U);
    const LinearBlurFilter* linearBlur = std::get_if<LinearBlurFilter>(&gradAngle.filter.operations.front());
    ASSERT_NE(linearBlur, nullptr);
    EXPECT_FLOAT_EQ(linearBlur->angleDegrees, 90.f);

    const ComputedStyle largeAngle = stylesheet.resolve("input", "", {});
    ASSERT_EQ(largeAngle.backgroundLayers.size(), 1U);
    ASSERT_NE(largeAngle.backgroundLayers.front().image.gradient(), nullptr);
    EXPECT_GE(largeAngle.backgroundLayers.front().image.gradient()->angleDegrees, 0.f);
    EXPECT_LT(largeAngle.backgroundLayers.front().image.gradient()->angleDegrees, 360.f);

    const ComputedStyle singleStop = stylesheet.resolve("floater", "", {});
    ASSERT_EQ(singleStop.backgroundLayers.size(), 1U);
    const Gradient* gradient = singleStop.backgroundLayers.front().image.gradient();
    ASSERT_NE(gradient, nullptr);
    ASSERT_EQ(gradient->stops.size(), 2U);
    EXPECT_EQ(gradient->stops[0].color, gradient->stops[1].color);
    EXPECT_FLOAT_EQ(gradient->stops[0].position, 0.f);
    EXPECT_FLOAT_EQ(gradient->stops[1].position, 1.f);
}

TEST(StyleSheet, FixesGradientStopPositions) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia("panel { background-image: linear-gradient(#fff -25%, #aaa 50%, #555 25% 125%); } "
                                             "floater { background-image: conic-gradient(#fff -90deg, #000 450deg); }");

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.warnings.empty());
    const ComputedStyle panel = stylesheet.resolve("panel", "", {});
    ASSERT_EQ(panel.backgroundLayers.size(), 1U);
    const Gradient* linear = panel.backgroundLayers.front().image.gradient();
    ASSERT_NE(linear, nullptr);
    ASSERT_EQ(linear->stops.size(), 4U);
    EXPECT_FLOAT_EQ(linear->stops[0].position, -.25f);
    EXPECT_FLOAT_EQ(linear->stops[1].position, .5f);
    EXPECT_FLOAT_EQ(linear->stops[2].position, .5f);
    EXPECT_FLOAT_EQ(linear->stops[3].position, 1.25f);

    const ComputedStyle floater = stylesheet.resolve("floater", "", {});
    ASSERT_EQ(floater.backgroundLayers.size(), 1U);
    const Gradient* conic = floater.backgroundLayers.front().image.gradient();
    ASSERT_NE(conic, nullptr);
    ASSERT_EQ(conic->stops.size(), 2U);
    EXPECT_FLOAT_EQ(conic->stops[0].position, -.25f);
    EXPECT_FLOAT_EQ(conic->stops[1].position, 1.25f);
}

TEST(StyleSheet, ParsesBackgroundMaskAndCursorLayers) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("i { background-image: url(icons/search.svg), linear-gradient(#fff, #000); "
                       "background-position: 10% 20%, right bottom; background-size: cover, 12px 14px; "
                       "background-repeat: no-repeat, repeat-x; background-origin: border-box, content-box; "
                       "background-clip: padding-box, border-box; background-attachment: fixed, local; "
                       "mask-image: url(icons/mask.svg); mask-mode: alpha; mask-position: right bottom; mask-size: contain; "
                       "mask-repeat: no-repeat; mask-origin: border-box; mask-clip: content-box; mask-composite: exclude; "
                       "mask-type: alpha; cursor: url(cursors/pointer.cur) 25 50, pointer; }")
            .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 2U);
    ASSERT_NE(style.backgroundLayers[0].image.resource(), nullptr);
    EXPECT_EQ(*style.backgroundLayers[0].image.resource(), "icons/search.svg");
    ASSERT_NE(style.backgroundLayers[1].image.gradient(), nullptr);
    EXPECT_NEAR(style.backgroundLayers[0].position.y.percent, .8f, 1.0e-4f);
    EXPECT_EQ(style.backgroundLayers[0].size.mode, BackgroundSizeType::Cover);
    EXPECT_EQ(style.backgroundLayers[1].size.mode, BackgroundSizeType::Explicit);
    EXPECT_EQ(style.backgroundLayers[0].repeat, BackgroundRepeat::NoRepeat);
    EXPECT_EQ(style.backgroundLayers[1].repeat, BackgroundRepeat::RepeatX);
    EXPECT_EQ(style.backgroundLayers[0].origin, BackgroundBox::BorderBox);
    EXPECT_EQ(style.backgroundLayers[0].clip, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Fixed);
    EXPECT_EQ(style.backgroundLayers[1].attachment, BackgroundAttachment::Local);

    ASSERT_EQ(style.maskLayers.size(), 1U);
    ASSERT_NE(style.maskLayers[0].image.image.resource(), nullptr);
    EXPECT_EQ(*style.maskLayers[0].image.image.resource(), "icons/mask.svg");
    EXPECT_EQ(style.maskLayers[0].mode, MaskMode::Alpha);
    EXPECT_EQ(style.maskLayers[0].composite, MaskComposite::Exclude);
    EXPECT_EQ(style.maskLayers[0].type, MaskType::Alpha);

    EXPECT_EQ(style.cursor, Core::Style::CursorStyle::Pointer);
    ASSERT_EQ(style.cursorImages.size(), 1U);
    EXPECT_EQ(style.cursorImages[0].resource, "cursors/pointer.cur");
    ASSERT_TRUE(style.cursorImages[0].hotspotX.has_value());
    ASSERT_TRUE(style.cursorImages[0].hotspotY.has_value());
    EXPECT_FLOAT_EQ(*style.cursorImages[0].hotspotX, 25.f);
    EXPECT_FLOAT_EQ(*style.cursorImages[0].hotspotY, 50.f);

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 3U);
    EXPECT_EQ(references[0].value, "icons/search.svg");
    EXPECT_FALSE(references[0].optional);
    EXPECT_EQ(references[1].value, "icons/mask.svg");
    EXPECT_TRUE(references[1].optional);
    EXPECT_EQ(references[2].value, "cursors/pointer.cur");
    EXPECT_FALSE(references[2].optional);
    EXPECT_TRUE(references[2].cursor);
}

TEST(StyleSheet, PublishesBorderImageResource) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { border-image-source: url(images/border.png); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_NE(style.borderImageSource().resource(), nullptr);
    EXPECT_EQ(*style.borderImageSource().resource(), "images/border.png");

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 1U);
    EXPECT_EQ(references.front().value, "images/border.png");
    EXPECT_FALSE(references.front().optional);
    EXPECT_FALSE(references.front().cursor);
}

TEST(StyleSheet, ParsesBorderImageGradient) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { border-image-source: linear-gradient(to right, #000, #fff); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    const Gradient* gradient = style.borderImageSource().gradient();
    ASSERT_NE(gradient, nullptr);
    EXPECT_EQ(gradient->kind, GradientKind::Linear);
    EXPECT_EQ(gradient->stops.size(), 2U);
}

TEST(StyleSheet, NegativeShadowSpread) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { box-shadow: 0 0 2px -4px #000; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    const auto& shadows = style.boxShadow();
    ASSERT_EQ(shadows.size(), 1U);
    EXPECT_FLOAT_EQ(shadows.front().blur, 2.f);
    EXPECT_FLOAT_EQ(shadows.front().spread, -4.f);
}

TEST(StyleSheet, PublishesDeferredResourceReferences) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia(":root { --background-image: url(icons/variable.svg); --mask-image: url(icons/variable-mask.svg); "
                       "--pointer: url(cursors/variable.cur), pointer; --border-image: url(images/variable-border.png); } "
                       "i { background-image: var(--background-image); mask-image: var(--mask-image); cursor: var(--pointer); "
                       "border-image-source: var(--border-image); }")
            .ok());

    const auto& references = stylesheet.resourceReferences();
    ASSERT_EQ(references.size(), 4U);
    EXPECT_EQ(references[0].value, "icons/variable.svg");
    EXPECT_FALSE(references[0].optional);
    EXPECT_EQ(references[1].value, "icons/variable-mask.svg");
    EXPECT_TRUE(references[1].optional);
    EXPECT_EQ(references[2].value, "cursors/variable.cur");
    EXPECT_FALSE(references[2].optional);
    EXPECT_TRUE(references[2].cursor);
    EXPECT_EQ(references[3].value, "images/variable-border.png");
    EXPECT_FALSE(references[3].optional);
    EXPECT_FALSE(references[3].cursor);
}

TEST(StyleSheet, ParsesMaskShorthand) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { mask: url(icons/mask.svg) alpha; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.maskLayers.size(), 1U);
    ASSERT_NE(style.maskLayers.front().image.image.resource(), nullptr);
    EXPECT_EQ(*style.maskLayers.front().image.image.resource(), "icons/mask.svg");
    EXPECT_EQ(style.maskLayers.front().mode, MaskMode::Alpha);
    EXPECT_EQ(style.maskLayers.front().composite, MaskComposite::Add);
    EXPECT_EQ(style.maskLayers.front().type, MaskType::Alpha);
}

TEST(StyleSheet, DecodesEscapedImageURLsAndFunctionNames) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(R"(i[data="a\\"] {} i { background-image: url(icons/a\20 b.svg); })").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    ASSERT_NE(style.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*style.backgroundLayers.front().image.resource(), "icons/a b.svg");

    StyleSheet escapedFunction;
    ASSERT_TRUE(escapedFunction.loadRadia(R"(i { background-image: u\72l(icons/search.svg); })").ok());
    ASSERT_EQ(escapedFunction.resolve("i", "", {}).backgroundLayers.size(), 1U);
    ASSERT_NE(escapedFunction.resolve("i", "", {}).backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*escapedFunction.resolve("i", "", {}).backgroundLayers.front().image.resource(), "icons/search.svg");
}

TEST(StyleSheet, DecodesEscapedValueFunctionsAndUnits) {
    constexpr char kStyles[] = R"(:root { --accent: #204060; } panel {
        background-color: v\61 r(--accent);
        filter: bl\75 r(4px);
        width: 17p\78;
    })";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    const ComputedStyle style = stylesheet.resolve("panel", "", {});
    EXPECT_NEAR(style.backgroundColor().resolvedColor().r, 32.f / 255.f, 1.0e-4f);
    ASSERT_EQ(style.filter.operations.size(), std::size_t(1));
    const BlurFilter* filter = std::get_if<BlurFilter>(&style.filter.operations.front());
    ASSERT_NE(filter, nullptr);
    EXPECT_EQ(filter->stdDeviation, 4.f);
    EXPECT_EQ(style.width().pixels(), 17.f);
}

TEST(StyleSheet, SkipsEmptyImageURL) {
    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(R"(i { background-image: url(""); })", "empty-url.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.property.value_invalid");
    EXPECT_TRUE(stylesheet.resourceReferences().empty());
}

TEST(StyleSheet, ParsesCSSCursorHotspotNumbers) {
    StyleSheet percentage;
    const auto percentageResult = percentage.loadRadia("i { cursor: url(cursors/default.png) 25% 50%, pointer; }");
    ASSERT_TRUE(percentageResult.ok());
    ASSERT_FALSE(percentageResult.warnings.empty());

    StyleSheet pixels;
    const auto pixelsResult = pixels.loadRadia("i { cursor: url(cursors/default.png) 25px 50px, pointer; }");
    ASSERT_TRUE(pixelsResult.ok());
    ASSERT_FALSE(pixelsResult.warnings.empty());

    StyleSheet outOfBounds;
    ASSERT_TRUE(outOfBounds.loadRadia("i { cursor: url(cursors/default.png) -1 1e30, pointer; }").ok());
    const ComputedStyle style = outOfBounds.resolve("i", "", {});
    ASSERT_EQ(style.cursorImages.size(), 1U);
    ASSERT_TRUE(style.cursorImages.front().hotspotX.has_value());
    ASSERT_TRUE(style.cursorImages.front().hotspotY.has_value());
    EXPECT_FLOAT_EQ(*style.cursorImages.front().hotspotX, -1.f);
    EXPECT_FLOAT_EQ(*style.cursorImages.front().hotspotY, 1e30f);

    StyleSheet incomplete;
    const auto incompleteResult = incomplete.loadRadia("i { cursor: url(cursors/default.png) 25, pointer; }");
    ASSERT_TRUE(incompleteResult.ok());
    ASSERT_FALSE(incompleteResult.warnings.empty());
}

TEST(StyleSheet, ParsesRasterBackgroundImages) {
    StyleSheet longhand;
    ASSERT_TRUE(longhand.loadRadia("i { background-image: url(images/pattern.png); }").ok());
    const ComputedStyle longhandStyle = longhand.resolve("i", "", {});
    ASSERT_EQ(longhandStyle.backgroundLayers.size(), 1U);
    ASSERT_NE(longhandStyle.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*longhandStyle.backgroundLayers.front().image.resource(), "images/pattern.png");

    StyleSheet shorthand;
    ASSERT_TRUE(shorthand.loadRadia("i { background: url(images/pattern.png); }").ok());
    const ComputedStyle shorthandStyle = shorthand.resolve("i", "", {});
    ASSERT_EQ(shorthandStyle.backgroundLayers.size(), 1U);
    ASSERT_NE(shorthandStyle.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*shorthandStyle.backgroundLayers.front().image.resource(), "images/pattern.png");
}

TEST(StyleSheet, ParsesBackgroundShorthandAfterSlash) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(icons/search.svg) center / contain no-repeat; }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    ASSERT_NE(style.backgroundLayers.front().image.resource(), nullptr);
    EXPECT_EQ(*style.backgroundLayers.front().image.resource(), "icons/search.svg");
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.x.percent, .5f);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.y.percent, .5f);
    EXPECT_EQ(style.backgroundLayers.front().size.mode, BackgroundSizeType::Contain);
    EXPECT_EQ(style.backgroundLayers.front().repeat, BackgroundRepeat::NoRepeat);
}

TEST(StyleSheet, UsesCSSImageInitialValues) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(icon.svg); mask-image: url(mask.svg); }").ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().r, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().g, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().b, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers[0].position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers[0].position.y.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers[0].size.mode, BackgroundSizeType::Auto);
    EXPECT_FALSE(style.backgroundLayers[0].size.width.has_value());
    EXPECT_FALSE(style.backgroundLayers[0].size.height.has_value());
    EXPECT_EQ(style.backgroundLayers[0].repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.backgroundLayers[0].origin, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers[0].clip, BackgroundBox::BorderBox);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Scroll);

    ASSERT_EQ(style.maskLayers.size(), 1U);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.position.y.percent, 1.f);
    EXPECT_EQ(style.maskLayers[0].image.size.mode, BackgroundSizeType::Auto);
    EXPECT_FALSE(style.maskLayers[0].image.size.width.has_value());
    EXPECT_FALSE(style.maskLayers[0].image.size.height.has_value());
    EXPECT_EQ(style.maskLayers[0].image.repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.maskLayers[0].image.origin, BackgroundBox::BorderBox);
    EXPECT_EQ(style.maskLayers[0].image.clip, BackgroundBox::BorderBox);
    EXPECT_EQ(style.maskLayers[0].type, MaskType::Alpha);
}

TEST(StyleSheet, BackgroundShorthandResetsAllComponents) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("i { background-color: #ff0000; background-position: right bottom; background-size: cover; "
                       "background-repeat: no-repeat; background-origin: content-box; background-clip: padding-box; "
                       "background-attachment: fixed; background: transparent; }")
            .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().r, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().g, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().b, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundColor().resolvedColor().a, 0.f);
    ASSERT_EQ(style.backgroundLayers.size(), 1U);
    EXPECT_EQ(style.backgroundLayers.front().image.value.index(), 0U);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.x.percent, 0.f);
    EXPECT_FLOAT_EQ(style.backgroundLayers.front().position.y.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers.front().size.mode, BackgroundSizeType::Auto);
    EXPECT_FALSE(style.backgroundLayers.front().size.width.has_value());
    EXPECT_FALSE(style.backgroundLayers.front().size.height.has_value());
    EXPECT_EQ(style.backgroundLayers.front().repeat, BackgroundRepeat::Repeat);
    EXPECT_EQ(style.backgroundLayers.front().attachment, BackgroundAttachment::Scroll);
    EXPECT_EQ(style.backgroundLayers.front().origin, BackgroundBox::PaddingBox);
    EXPECT_EQ(style.backgroundLayers.front().clip, BackgroundBox::BorderBox);
}

TEST(StyleSheet, SkipsDuplicateImageLayerComponents) {
    const char* invalid[] = {
        "i { background: url(a.svg) repeat repeat; }",
        "i { background: url(a.svg) fixed scroll; }",
        "i { mask: url(a.svg) alpha alpha; }",
        "i { mask: url(a.svg) add add; }",
    };
    for (const char* styles : invalid) {
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(styles);
        EXPECT_TRUE(result.ok()) << styles;
        EXPECT_FALSE(result.warnings.empty()) << styles;
    }

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia("i { background: url(a.svg) repeat no-repeat; }").ok());
    EXPECT_EQ(stylesheet.resolve("i", "", {}).backgroundLayers.front().repeat, BackgroundRepeat::RepeatX);
}

TEST(StyleSheet, NormalizesImageLonghandsRegardlessOfDeclarationOrder) {
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet
            .loadRadia("i { background-position: right, left; background-attachment: fixed, local; "
                       "background-image: url(one.svg), url(two.svg); mask-position: right, left; "
                       "mask-image: url(one.svg), url(two.svg); }")
            .ok());

    const ComputedStyle style = stylesheet.resolve("i", "", {});
    ASSERT_EQ(style.backgroundLayers.size(), 2U);
    EXPECT_EQ(style.backgroundLayers[0].position.x.percent, 1.f);
    EXPECT_EQ(style.backgroundLayers[1].position.x.percent, 0.f);
    EXPECT_EQ(style.backgroundLayers[0].attachment, BackgroundAttachment::Fixed);
    EXPECT_EQ(style.backgroundLayers[1].attachment, BackgroundAttachment::Local);
    ASSERT_EQ(style.maskLayers.size(), 2U);
    EXPECT_EQ(style.maskLayers[0].image.position.x.percent, 1.f);
    EXPECT_EQ(style.maskLayers[1].image.position.x.percent, 0.f);
}

TEST(StyleSheet, DoesNotApplyInheritedOpacityToMaskCoverage) {
    ComputedStyle style;
    style.maskLayers = {MaskLayer {}};
    style.maskLayers[0].image.image = Image {Gradient {}};
    style.maskLayers[0].image.image.gradient()->stops = {{Color(1.f, 1.f, 1.f, .8f), 0.f}, {Color(1.f, 1.f, 1.f, .4f), 1.f}};

    resolveStyleColors(style, Color {});
    Core::Style::applyOpacity(style, .5f);

    EXPECT_FLOAT_EQ(style.opacity().value, .5f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.image.gradient()->stops[0].color.resolvedColor().a, .8f);
    EXPECT_FLOAT_EQ(style.maskLayers[0].image.image.gradient()->stops[1].color.resolvedColor().a, .4f);
}

TEST(StyleSheet, SkipsInvalidFilters) {
    const char* invalidSources[] = {
        "panel { background-color: linear-gradient(#fff); }",
        "panel { background-color: radial-gradient(square, #fff, #000); }",
        "panel { border-color: conic-gradient(from nowhere, #fff, #000); }",
        "panel { border: 1px repeating-linear-gradient(#fff 20%, #000 20%); }",
        "panel { box-shadow: 0 0 -1px #000; }",
        "panel { outline: 10% #000; }",
        "panel { outline: 0% #000; }",
        "panel { outline: 1px 2px #000; }",
        "panel { outline-offset: 0%; }",
        "panel { outline: 1px dashed solid #000; }",
        "panel { outline: #000 auto 1px; }",
        "panel { outline: -focus-ring-color auto 1px; }",
        "panel { filter: blur(4); }",
        "panel { filter: blur(-1px); }",
        "panel { filter: blur(1%); }",
        "panel { filter: blur(0%); }",
        "panel { filter: blur(); }",
        "panel { filter: blur(1px, 2px); }",
        "panel { box-shadow: 0% 0% #000; }",
        "panel { filter: brightness(1); }",
        "panel { filter: blur(1px) none; }",
        "panel { filter: linear-blur(to nowhere, 0px 0%, 4px 100%); }",
        "panel { filter: linear-blur(0px 75%, 4px 25%); }",
        "panel { filter: linear-blur(0px 0%, 4px 50%, 8px 100%); }",
    };
    constexpr char kLargeBlurStyles[] = "panel { filter: blur(64px); }";

    for (const char* source : invalidSources) {
        SCOPED_TRACE(::testing::Message() << "invalid filter: " << source);
        StyleSheet stylesheet;
        const auto result = stylesheet.loadRadia(source, "effects.css");
        ASSERT_TRUE(result.ok());
        ASSERT_FALSE(result.warnings.empty());
    }

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kLargeBlurStyles, "large-filter.css").ok());
}

TEST(StyleSheet, CopiesStylesheetState) {
    constexpr char kOriginalStyles[] = "panel { width: 10px; }";
    constexpr char kReplacementStyles[] = "panel { width: 20px; }";

    StyleSheet original;
    ASSERT_TRUE(original.loadRadia(kOriginalStyles, "original.css").ok());
    const std::uint64_t copiedGeneration = original.generation();

    StyleSheet copy = original;
    ASSERT_TRUE(original.loadRadia(kReplacementStyles, "replacement.css").ok());
    EXPECT_EQ(copy.resolve("panel", "", {}).width().pixels(), 10.f);
    EXPECT_EQ(copy.generation(), copiedGeneration);
    EXPECT_EQ(original.resolve("panel", "", {}).width().pixels(), 20.f);

    StyleSheet assigned;
    assigned = copy;
    EXPECT_EQ(assigned.resolve("panel", "", {}).width().pixels(), 10.f);

    StyleSheet moved = std::move(assigned);
    EXPECT_EQ(moved.resolve("panel", "", {}).width().pixels(), 10.f);
}

TEST(StyleSheet, MergesStyleLayersTransactionally) {
    constexpr char kBaseLayerStyles[] = "panel { width: 10px; height: 30px; }";
    constexpr char kDerivedLayerStyles[] = "panel { width: 20px; }";
    constexpr char kMalformedLayerStyles[] = "not a rule";

    StyleSheet stylesheet;
    const std::vector<StyleLayer> layers {
        {StyleOrigin::Skin, {"base/skin.css", kBaseLayerStyles}},
        {StyleOrigin::Skin, {"derived/skin.css", kDerivedLayerStyles}},
    };
    ASSERT_TRUE(stylesheet.loadRadiaLayers(layers).ok());
    const ComputedStyle resolved = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(resolved.width().pixels(), 20.f);
    EXPECT_EQ(resolved.height().pixels(), 30.f);

    const auto malformed = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"base/skin.css", kBaseLayerStyles}},
        {StyleOrigin::Skin, {"derived/skin.css", kMalformedLayerStyles}},
    });
    ASSERT_TRUE(malformed.ok());
    ASSERT_FALSE(malformed.warnings.empty());
    EXPECT_EQ(malformed.warnings.front().source, "derived/skin.css");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 10.f);
}

TEST(StyleSheet, OverridesDefaultRule) {
    const std::vector<StyleLayer> layers {
        {StyleOrigin::Skin, {"skin.css", "panel { width: 20px; }"}},
        {StyleOrigin::UserAgent, {"ua.css", "panel.primary { width: 10px; }"}},
    };

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers(layers).ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"primary"}).width().pixels(), 20.f);
}

TEST(StyleSheet, RestrictsInternalAlignment) {
    StyleSheet stylesheet;
    const auto skinOnly = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"skin.css", "button { -internal-align-content-block: center; }"}},
    });
    ASSERT_TRUE(skinOnly.ok());
    ASSERT_EQ(skinOnly.warnings.size(), std::size_t(1));
    EXPECT_EQ(skinOnly.warnings.front().code, "stylesheet.property.ua_only");
    EXPECT_FALSE(stylesheet.resolve("button", "", {}).alignContentBlockCenter);

    const auto defaultAndSkin = stylesheet.loadRadiaLayers({
        {StyleOrigin::Skin, {"skin.css", "button { -internal-align-content-block: normal; }"}},
        {StyleOrigin::UserAgent, {"ua.css", "button { -internal-align-content-block: center; }"}},
    });
    ASSERT_TRUE(defaultAndSkin.ok());
    ASSERT_EQ(defaultAndSkin.warnings.size(), std::size_t(1));
    EXPECT_EQ(defaultAndSkin.warnings.front().code, "stylesheet.property.ua_only");
    EXPECT_TRUE(stylesheet.resolve("button", "", {}).alignContentBlockCenter);
}

TEST(StyleSheet, ResolvesRecursiveImports) {
    constexpr char kEntrypointStyles[] = "@import \"components/panel.css\";\n"
                                         ":root { --panel-width: 12px; }\n"
                                         "panel { width: var(--panel-width); }\n"
                                         "panel { width: 30px; }";
    constexpr char kPanelModule[] = "@import \"../foundation/sizes.css\";\n"
                                    "panel { width: var(--panel-width); height: var(--panel-height); }";
    constexpr char kFoundationModuleStyles[] = ":root { --panel-height: 18px; }";

    StyleSheet stylesheet;
    ResourceLayer layer {"theme/main.css", kEntrypointStyles};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"foundation/sizes.css", kFoundationModuleStyles},
        {"components/panel.css", kPanelModule},
    };

    ASSERT_TRUE(stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer}}).ok());
    const ComputedStyle resolved = stylesheet.resolve("panel", "", {});
    EXPECT_EQ(resolved.width().pixels(), 30.f);
    EXPECT_EQ(resolved.height().pixels(), 18.f);

    const auto& dependencies = stylesheet.dependencies();
    ASSERT_TRUE(dependencies.contains("theme/main.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/panel.css"));
    ASSERT_TRUE(dependencies.contains("theme/components/panel.css"));
    EXPECT_TRUE(dependencies.at("theme/components/panel.css").contains("theme/foundation/sizes.css"));
}

TEST(StyleSheet, ImportOrder) {
    constexpr char kEntrypointStyles[] = "@import \"first.css\"; @import \"second.css\";";

    ResourceLayer layer {"theme/main.css", kEntrypointStyles};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"first.css", ".foo { width: 11px; }"},
        {"second.css", ".foo { width: 22px; }"},
    };

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer}}).ok());
    EXPECT_EQ(stylesheet.resolve("panel", "", {"foo"}).width().pixels(), 22.f);
}

TEST(StyleSheet, AcceptsStringAndURLImportTargets) {
    constexpr char kEntrypointStyles[] = R"(@impor\74 "components/a\2e css";
@import url(components/b.css);
@import url("components/c.css");
panel { width: 30px; })";

    StyleSheet stylesheet;
    ResourceLayer layer {"theme/main.css", kEntrypointStyles};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"components/a.css", "panel { height: 10px; }"},
        {"components/b.css", "panel { min-width: 11px; }"},
        {"components/c.css", "panel { max-width: 12px; }"},
    };

    const auto result = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer}});
    ASSERT_TRUE(result.ok());
    const auto& dependencies = stylesheet.dependencies();
    ASSERT_TRUE(dependencies.contains("theme/main.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/a.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/b.css"));
    EXPECT_TRUE(dependencies.at("theme/main.css").contains("theme/components/c.css"));
}

TEST(StyleSheet, SkipsUnsupportedImportConditionsWithoutLoading) {
    constexpr char kStyles[] = "@import url(does-not-exist.css) screen; panel { width: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, {"theme/main.css", kStyles}}});
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheet, KeepsLaterImportsAfterUnsupportedConditions) {
    constexpr char kStyles[] = "@import \"skipped.css\" screen; @import \"valid.css\"; panel { width: 13px; }";

    ResourceLayer layer {"theme/main.css", kStyles};
    layer.entrypoint = "main.css";
    layer.modules = {{"valid.css", "panel { height: 17px; }"}};

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer}});

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_TRUE(stylesheet.dependencies().at("theme/main.css").contains("theme/valid.css"));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 17.f);
}

TEST(StyleSheet, DoesNotTreatCDOAsImportWhitespace) {
    constexpr char kStyles[] = "@import \"missing.css\" <!--; panel { width: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kStyles, "import.css");

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.unsupported");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
}

TEST(StyleSheet, RecoversMalformedImportAtRuleAtItsBoundary) {
    constexpr char kStyles[] = "@import \"missing.css\" panel { width: 99px; } label { height: 13px; }";

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadia(kStyles, "theme/main.css");
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(1));
    EXPECT_EQ(result.warnings.front().code, "stylesheet.import.syntax");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}).width().isAuto());
    EXPECT_EQ(stylesheet.resolve("label", "", {}).height().pixels(), 13.f);
}

TEST(StyleSheet, KeepsLaterImportsAfterMalformedImportAtRules) {
    constexpr char kStyles[] = "@import \"broken.css\" panel { width: 99px; } @import ???; @import \"\"; "
                               "@import \"valid.css\"; panel { width: 13px; }";

    ResourceLayer layer {"theme/main.css", kStyles};
    layer.entrypoint = "main.css";
    layer.modules = {{"valid.css", "panel { height: 17px; }"}};

    StyleSheet stylesheet;
    const auto result = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer}});

    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(3));
    for (const auto& warning : result.warnings)
        EXPECT_EQ(warning.code, "stylesheet.import.syntax");
    EXPECT_TRUE(stylesheet.dependencies().at("theme/main.css").contains("theme/valid.css"));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 13.f);
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).height().pixels(), 17.f);
}

TEST(StyleSheet, PreservesStylesheetOnImportFailure) {
    constexpr char kBaselineStyles[] = "panel { width: 44px; }";
    constexpr char kMissingImport[] = "\n@import \"missing.css\";";
    constexpr char kCycleImport[] = "@import \"cycle.css\";";
    constexpr char kCycleModule[] = "@import \"main.css\";";
    constexpr char kTraversalImport[] = "@import \"../outside.css\";";
    constexpr char kMalformedImport[] = "@import \"broken.css\";";
    constexpr char kMalformedModule[] = "panel { width: ; }";
    constexpr char kLateImport[] = "panel { width: 1px; } @import \"late.css\";";
    constexpr char kLateModule[] = "panel { height: 2px; }";

    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kBaselineStyles).ok());

    auto layer = [](std::string source) {
        ResourceLayer result {"theme/main.css", std::move(source)};
        result.entrypoint = "main.css";
        return result;
    };

    auto missing = layer(kMissingImport);
    const auto missingResult = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, missing}});
    ASSERT_FALSE(missingResult.ok());
    ASSERT_FALSE(missingResult.errors.empty());
    EXPECT_EQ(missingResult.errors.front().code, "stylesheet.import.missing");
    EXPECT_EQ(missingResult.errors.front().line, std::size_t(2));
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 44.f);

    auto cycle = layer(kCycleImport);
    cycle.modules["cycle.css"] = kCycleModule;
    const auto cycleResult = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, cycle}});
    ASSERT_FALSE(cycleResult.ok());
    ASSERT_FALSE(cycleResult.errors.empty());
    EXPECT_EQ(cycleResult.errors.front().code, "stylesheet.import.cycle");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 44.f);

    const auto traversalResult = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer(kTraversalImport)}});
    ASSERT_FALSE(traversalResult.ok());
    ASSERT_FALSE(traversalResult.errors.empty());
    EXPECT_EQ(traversalResult.errors.front().code, "stylesheet.import.path_invalid");
    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 44.f);

    auto malformed = layer(kMalformedImport);
    malformed.modules["broken.css"] = kMalformedModule;
    const auto malformedResult = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, malformed}});
    ASSERT_TRUE(malformedResult.ok());
    ASSERT_FALSE(malformedResult.warnings.empty());
    EXPECT_EQ(malformedResult.warnings.front().source, "theme/broken.css");
    EXPECT_TRUE(stylesheet.resolve("panel", "", {}).width().isAuto());

    auto late = layer(kLateImport);
    late.modules["late.css"] = kLateModule;
    const auto lateResult = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, late}});
    ASSERT_TRUE(lateResult.ok());
    ASSERT_FALSE(lateResult.warnings.empty());
    EXPECT_EQ(lateResult.warnings.front().code, "stylesheet.import.order");

    EXPECT_EQ(stylesheet.resolve("panel", "", {}).width().pixels(), 1.f);
}

TEST(StyleSheet, ReportsEachSharedImportWarning) {
    constexpr char kEntrypointImports[] = "@import \"branch-a.css\"; @import \"branch-b.css\";";
    constexpr char kBranchA[] = "@import \"shared.css\"; panel { width: 10px; }";
    constexpr char kBranchB[] = "@import \"shared.css\"; panel { height: 20px; }";
    constexpr char kSharedFailure[] = "panel { unknown-property: 1; }";

    StyleSheet stylesheet;
    ResourceLayer layer {"theme/main.css", kEntrypointImports};
    layer.entrypoint = "main.css";
    layer.modules = {
        {"branch-a.css", kBranchA},
        {"branch-b.css", kBranchB},
        {"shared.css", kSharedFailure},
    };

    const auto result = stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, layer}});
    ASSERT_TRUE(result.ok());
    ASSERT_EQ(result.warnings.size(), std::size_t(2));
    EXPECT_EQ(result.warnings[0].source, "theme/shared.css");
    EXPECT_EQ(result.warnings[1].source, "theme/shared.css");
    EXPECT_NE(result.warnings[0].message.find("Import chain:"), std::string::npos);
    EXPECT_NE(result.warnings[1].message.find("Import chain:"), std::string::npos);
}

TEST(StyleSheet, FontFaceOrder) {
    constexpr char kEntrypoint[] = R"css(@import "typography.css";
@font-face { font-family: MainSans; src: url(fonts/main.bin); }
@font-face { font-family: "Main Sans Bold"; src: url("fonts/main-bold.bin"); font-weight: bold; }
)css";
    constexpr char kTypography[] = R"css(@font-face {
  font-family: "Imported Sans";
  src: url("fonts/imported.woff2") format("woff2"), url(fonts/fallback.bin);
  font-style: italic;
  font-weight: 650;
  font-width: 87.5%;
}
)css";

    ResourceLayer layer {"theme/skin.css", kEntrypoint, "skin.css", {{"typography.css", kTypography}}};
    StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadiaLayers({StyleLayer {StyleOrigin::Skin, std::move(layer)}}).ok());

    const std::vector<Core::CSS::FontFace>& fontFaces = stylesheet.fontFaces();
    ASSERT_EQ(fontFaces.size(), 3U);
    EXPECT_EQ(fontFaces[0].family, "Imported Sans");
    EXPECT_EQ(fontFaces[0].selection.style, Core::Style::FontStyle::Italic);
    EXPECT_FLOAT_EQ(fontFaces[0].selection.weight.value, 650.f);
    EXPECT_FLOAT_EQ(fontFaces[0].selection.width.percentage, 87.5f);
    ASSERT_EQ(fontFaces[0].sources.size(), 2U);
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(fontFaces[0].sources[0].value).url, "fonts/imported.woff2");
    EXPECT_EQ(fontFaces[0].sources[0].sourceName, "theme/typography.css");
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(fontFaces[0].sources[1].value).url, "fonts/fallback.bin");
    EXPECT_EQ(fontFaces[1].family, "MainSans");
    EXPECT_EQ(fontFaces[2].family, "Main Sans Bold");
    EXPECT_FLOAT_EQ(fontFaces[2].selection.weight.value, 700.f);
}

TEST(StyleSheet, FontSources) {
    constexpr char kStyles[] = R"css(
@font-face {
  font-family: Supported;
  src: local("Missing"), url(unknown.bin) format("future-format"),
       url(fallback.woff2) format(woff2) tech(features-opentype, color-COLRv1),
       url(variations.woff2) tech(variations);
}
)css";
    constexpr char kUnsupported[] = "@font-face { font-family: Unsupported; src: url(unsupported.bin) format(\"future-format\"); }";
    constexpr char kMalformed[] = "@font-face { font-family: Malformed; src: url(malformed.woff2) unexpected; }";

    Core::CSS::StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());

    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    EXPECT_EQ(stylesheet.fontFaces().front().family, "Supported");
    ASSERT_EQ(stylesheet.fontFaces().front().sources.size(), 2U);
    ASSERT_TRUE(std::holds_alternative<Core::CSS::FontFaceLocal>(stylesheet.fontFaces().front().sources.front().value));
    EXPECT_EQ(std::get<Core::CSS::FontFaceLocal>(stylesheet.fontFaces().front().sources.front().value).name, "Missing");
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(stylesheet.fontFaces().front().sources.back().value).url, "fallback.woff2");

    const Core::CSS::StyleSheetLoadResult rejected = stylesheet.loadRadia(kUnsupported);
    ASSERT_FALSE(rejected.ok());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "stylesheet.font_face.source_unsupported");
    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    EXPECT_EQ(stylesheet.fontFaces().front().family, "Supported");

    const Core::CSS::StyleSheetLoadResult malformed = stylesheet.loadRadia(kMalformed);
    ASSERT_FALSE(malformed.ok());
    ASSERT_FALSE(malformed.errors.empty());
    EXPECT_EQ(malformed.errors.front().code, "stylesheet.font_face.source_unsupported");
    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    EXPECT_EQ(stylesheet.fontFaces().front().family, "Supported");
}

TEST(StyleSheet, LocalFontSources) {
    constexpr char kStyles[] = "@font-face { font-family: Local; src: local(\"Full Name\"), local(PostScript Name), url(local.woff2); }";

    Core::CSS::StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    ASSERT_EQ(stylesheet.fontFaces().size(), 1U);
    const auto& sources = stylesheet.fontFaces().front().sources;
    ASSERT_EQ(sources.size(), 3U);
    ASSERT_TRUE(std::holds_alternative<Core::CSS::FontFaceLocal>(sources[0].value));
    EXPECT_EQ(std::get<Core::CSS::FontFaceLocal>(sources[0].value).name, "Full Name");
    ASSERT_TRUE(std::holds_alternative<Core::CSS::FontFaceLocal>(sources[1].value));
    EXPECT_EQ(std::get<Core::CSS::FontFaceLocal>(sources[1].value).name, "PostScript Name");
    ASSERT_TRUE(std::holds_alternative<Core::CSS::FontFaceURL>(sources[2].value));
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(sources[2].value).url, "local.woff2");
}

TEST(StyleSheet, FontFaceMatchOrder) {
    constexpr char kStyles[] = R"css(
@font-face { font-family: Match; src: url(narrow-bold.woff2); font-weight: 700; font-width: 90%; }
@font-face { font-family: Match; src: url(italic.woff2); font-style: italic; font-weight: 700; }
@font-face { font-family: Match; src: url(weight-500.woff2); font-weight: 500; }
@font-face { font-family: Match; src: url(first-400.woff2); font-weight: 400; }
@font-face { font-family: Match; src: url(last-400.woff2); font-weight: 400; }
)css";

    Core::CSS::StyleSheet stylesheet;
    ASSERT_TRUE(stylesheet.loadRadia(kStyles).ok());
    const Core::Style::FontSelectionRequest request {Core::Style::FontWeight {700.f}, Core::Style::FontWidth {100.f},
        Core::Style::FontStyle::Normal};

    const std::vector<const Core::CSS::FontFace*> faces = Core::CSS::fontFacesInMatchOrder(stylesheet.fontFaces(), "match", request);

    ASSERT_EQ(faces.size(), 5U);
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(faces[0]->sources[0].value).url, "weight-500.woff2");
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(faces[1]->sources[0].value).url, "last-400.woff2");
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(faces[2]->sources[0].value).url, "first-400.woff2");
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(faces[3]->sources[0].value).url, "italic.woff2");
    EXPECT_EQ(std::get<Core::CSS::FontFaceURL>(faces[4]->sources[0].value).url, "narrow-bold.woff2");
}
