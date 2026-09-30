/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "ComputedStyle.h"
#include <algorithm>
#include <array>
#include <cstdint>
#include "ComputedStyleProperties.h"
#include "StyleProperty.h"

namespace Core::Style {
using enum CSS::KeywordName;

namespace {
struct SystemColor {
    CSS::KeywordName keyword;
    std::uint32_t light;
    std::uint32_t dark;
};

const std::array kSystemColors {
    SystemColor {KeywordAccentColor, 0x0067c0, 0x8ab4f8},
    SystemColor {KeywordAccentColorText, 0xffffff, 0x202124},
    SystemColor {KeywordActiveText, 0xee0000, 0xf28b82},
    SystemColor {KeywordButtonBorder, 0x767676, 0x5f6368},
    SystemColor {KeywordButtonFace, 0xf0f0f0, 0x3c4043},
    SystemColor {KeywordButtonText, 0x000000, 0xe8eaed},
    SystemColor {KeywordCanvas, 0xffffff, 0x1c1c1c},
    SystemColor {KeywordCanvasText, 0x000000, 0xf5f5f5},
    SystemColor {KeywordField, 0xffffff, 0x303134},
    SystemColor {KeywordFieldText, 0x000000, 0xe8eaed},
    SystemColor {KeywordGrayText, 0x6d6d6d, 0x9aa0a6},
    SystemColor {KeywordHighlight, 0xb4d5ff, 0x2b65a8},
    SystemColor {KeywordHighlightText, 0x000000, 0xffffff},
    SystemColor {KeywordLinkText, 0x0000ee, 0x8ab4f8},
    SystemColor {KeywordMark, 0xffff00, 0x5c4a00},
    SystemColor {KeywordMarkText, 0x000000, 0xffeb3b},
    SystemColor {KeywordSelectedItem, 0x3875d7, 0x174ea6},
    SystemColor {KeywordSelectedItemText, 0xffffff, 0xffffff},
    SystemColor {KeywordVisitedText, 0x551a8b, 0xc58af9},
};
} // namespace

namespace {
bool explicitlyInherits(const ComputedStyle& style, std::string_view propertyName) {
    return std::find(style.explicitlyInheritedProperties.begin(), style.explicitlyInheritedProperties.end(), propertyName)
        != style.explicitlyInheritedProperties.end();
}

bool specifiedHere(const ComputedStyle& style, std::string_view propertyName) {
    return std::find(style.specifiedProperties.begin(), style.specifiedProperties.end(), propertyName) != style.specifiedProperties.end();
}

bool shouldInherit(const ComputedStyle& style, std::string_view property) {
    return explicitlyInherits(style, property) || (detail::isPropertyInherited(property) && !specifiedHere(style, property));
}
} // namespace

void resolveStyleColors(ComputedStyle& style, const Core::Color& inheritedColor) {
    Color color = style.color();
    color.resolve(inheritedColor, style.usedColorScheme);
    style.setColor(std::move(color));
    const Core::Color currentColor = style.color().resolvedColor();
    Color backgroundColor = style.backgroundColor();
    backgroundColor.resolve(currentColor, style.usedColorScheme);
    style.setBackgroundColor(std::move(backgroundColor));
    Layout::RectEdges<Color> borderColors = style.borderColor();
    borderColors.top.resolve(currentColor, style.usedColorScheme);
    borderColors.right.resolve(currentColor, style.usedColorScheme);
    borderColors.bottom.resolve(currentColor, style.usedColorScheme);
    borderColors.left.resolve(currentColor, style.usedColorScheme);
    style.setBorderColor(std::move(borderColors));
    if (auto* color = std::get_if<Color>(&style.stroke))
        color->resolve(currentColor, style.usedColorScheme);
    AccentColor accent = style.accentColor();
    if (!accent.isKeyword())
        accent.value().resolve(currentColor, style.usedColorScheme);
    style.setAccentColor(std::move(accent));
    ScrollbarColor scrollbarColor = style.scrollbarColor();
    scrollbarColor.thumb.resolve(currentColor, style.usedColorScheme);
    scrollbarColor.track.resolve(currentColor, style.usedColorScheme);
    style.setScrollbarColor(std::move(scrollbarColor));
    for (BackgroundLayer& layer : style.backgroundLayers)
        if (Gradient* gradient = layer.image.gradient())
            for (GradientStop& stop : gradient->stops)
                stop.color.resolve(currentColor, style.usedColorScheme);
    for (MaskLayer& layer : style.maskLayers)
        if (Gradient* gradient = layer.image.image.gradient())
            for (GradientStop& stop : gradient->stops)
                stop.color.resolve(currentColor, style.usedColorScheme);
    if (auto* image = std::get_if<Image>(&style.stroke); image && image->gradient()) {
        Gradient* gradient = image->gradient();
        for (GradientStop& stop : gradient->stops)
            stop.color.resolve(currentColor, style.usedColorScheme);
    }
    BoxShadows shadows = style.boxShadow();
    for (BoxShadow& shadow : shadows)
        shadow.color.resolve(currentColor, style.usedColorScheme);
    style.setBoxShadow(std::move(shadows));
    style.outline().color.resolve(currentColor, style.usedColorScheme);
}

void normalizeOverflow(ComputedStyle& style) {
    const auto scrollable = [](Overflow value) {
        return value == Overflow::Hidden || value == Overflow::Scroll || value == Overflow::Auto;
    };
    if (scrollable(style.overflowY())) {
        if (style.overflowX() == Overflow::Visible)
            style.setOverflowX(Overflow::Auto);
        else if (style.overflowX() == Overflow::Clip)
            style.setOverflowX(Overflow::Hidden);
    }
    if (scrollable(style.overflowX())) {
        if (style.overflowY() == Overflow::Visible)
            style.setOverflowY(Overflow::Auto);
        else if (style.overflowY() == Overflow::Clip)
            style.setOverflowY(Overflow::Hidden);
    }
}

void inheritStyle(ComputedStyle& style, const ComputedStyle& parent) {
    BuilderState builderState {style, &parent};
    for (const detail::PropertyDefinition* property = detail::legacyPropertyBegin(); property != detail::legacyPropertyEnd(); ++property)
        if (property->inherit && shouldInherit(style, property->name))
            property->inherit(style, parent);

    for (std::size_t index = 0; index < static_cast<std::size_t>(CSS::Property::Count); ++index) {
        const CSS::Property property = static_cast<CSS::Property>(index);
        const std::string_view name = CSS::propertyName(property);
        const auto* legacy = detail::findLegacyProperty(name);
        if ((legacy && legacy->inherit) || !shouldInherit(style, name))
            continue;
        applyProperty(property, builderState, ApplyType::Inherit);
    }
    style.specifiedProperties.clear();
    style.explicitlyInheritedProperties.clear();
    style.textDecorationPropagation = static_cast<TextDecoration>(
        static_cast<unsigned>(parent.textDecorationPropagation) | static_cast<unsigned>(style.textDecoration()));
}

void applyOpacity(ComputedStyle& style, float inheritedOpacity) {
    const float opacity = inheritedOpacity * style.opacity().value;
    auto applyAlpha = [opacity](Color color) {
        color.resolvedColor().a *= opacity;
        return color;
    };
    style.setBackgroundColor(applyAlpha(style.backgroundColor()));
    Layout::RectEdges<Color> borderColors = style.borderColor();
    borderColors.top.resolvedColor().a *= opacity;
    borderColors.right.resolvedColor().a *= opacity;
    borderColors.bottom.resolvedColor().a *= opacity;
    borderColors.left.resolvedColor().a *= opacity;
    style.setBorderColor(std::move(borderColors));
    Color color = style.color();
    color.resolvedColor().a *= opacity;
    style.setColor(std::move(color));
    if (auto* color = std::get_if<Color>(&style.stroke))
        color->resolvedColor().a *= opacity;
    style.outline().color.resolvedColor().a *= opacity;
    ScrollbarColor scrollbarColor = style.scrollbarColor();
    if (!scrollbarColor.automatic) {
        scrollbarColor.thumb.resolvedColor().a *= opacity;
        scrollbarColor.track.resolvedColor().a *= opacity;
        style.setScrollbarColor(std::move(scrollbarColor));
    }
    BoxShadows shadows = style.boxShadow();
    for (BoxShadow& shadow : shadows)
        shadow.color.resolvedColor().a *= opacity;
    style.setBoxShadow(std::move(shadows));
    for (BackgroundLayer& layer : style.backgroundLayers)
        if (Gradient* gradient = layer.image.gradient())
            for (GradientStop& stop : gradient->stops)
                stop.color.resolvedColor().a *= opacity;
    if (auto* image = std::get_if<Image>(&style.stroke); image && image->gradient()) {
        Gradient* gradient = image->gradient();
        for (GradientStop& stop : gradient->stops)
            stop.color.resolvedColor().a *= opacity;
    }
    style.setOpacity(Opacity {opacity});
}

Core::Color systemColorValue(CSS::KeywordName keyword, ColorSchemeMode scheme) {
    const auto found = std::find_if(kSystemColors.begin(), kSystemColors.end(), [keyword](const SystemColor& color) {
        return color.keyword == keyword;
    });
    if (found == kSystemColors.end())
        return Core::Color(0.f, 0.f, 0.f);
    const std::uint32_t rgb = scheme == ColorSchemeMode::Light ? found->light : found->dark;
    constexpr float scale = 1.f / 255.f;
    return Core::Color {((rgb >> 16) & 0xff) * scale, ((rgb >> 8) & 0xff) * scale, (rgb & 0xff) * scale};
}

Color Color::current() { return Color(KeywordCurrentColor); }

Color Color::fromKeyword(CSS::KeywordName keyword) {
    if (keyword == KeywordTransparent)
        return fromColor(Core::Color(0.f, 0.f, 0.f, 0.f));
    if (keyword == KeywordCurrentColor || isSystemColorKeyword(keyword))
        return Color(keyword);
    return fromColor(Core::Color(0.f, 0.f, 0.f));
}

bool Color::isCurrentColor() const {
    const auto* keyword = std::get_if<CSS::KeywordName>(&mValue);
    return keyword && *keyword == KeywordCurrentColor;
}

bool Color::isSystemColor() const {
    const auto* keyword = std::get_if<CSS::KeywordName>(&mValue);
    return keyword && isSystemColorKeyword(*keyword);
}

std::optional<Color> Color::fromCSS(const CSS::Color& value, ColorSchemeMode scheme) {
    const CSS::Color* selected = &value;
    while (const CSS::LightDarkColor* lightDark = selected->lightDark())
        selected = scheme == ColorSchemeMode::Light ? &lightDark->light : &lightDark->dark;

    if (const Core::Color* color = selected->solidColor())
        return fromColor(*color);
    if (const CSS::Keyword* keyword = selected->keyword()) {
        if (keyword->keyword == KeywordCurrentColor)
            return Color(keyword->keyword);
        if (isSystemColorKeyword(keyword->keyword))
            return fromColor(systemColorValue(keyword->keyword, scheme));
    }
    return std::nullopt;
}

void Color::resolve(const Core::Color& currentColorValue, ColorSchemeMode scheme) {
    const auto* keyword = std::get_if<CSS::KeywordName>(&mValue);
    if (!keyword)
        return;
    if (*keyword == KeywordCurrentColor)
        mValue = currentColorValue;
    else if (isSystemColorKeyword(*keyword))
        mValue = systemColorValue(*keyword, scheme);
}

ComputedStyle::ComputedStyle()
    : mInheritedData(InheritedData::create())
    , mInheritedRareData(InheritedRareData::create())
    , mNonInheritedData(NonInheritedData::create()) {
    mInheritedFlags.textAlign = static_cast<unsigned>(initialTextAlign());
    mInheritedFlags.visibility = static_cast<unsigned>(initialVisibility());
    mInheritedFlags.pointerEvents = static_cast<unsigned>(initialPointerEvents());
    mInheritedFlags.textWrapMode = static_cast<unsigned>(initialTextWrapMode());
    mInheritedFlags.textWrapStyle = static_cast<unsigned>(initialTextWrapStyle());

    mNonInheritedFlags.internalAlignContentBlock = false;
    mNonInheritedFlags.boxSizing = static_cast<unsigned>(initialBoxSizing());
    mNonInheritedFlags.display = static_cast<unsigned>(initialDisplay());
    mNonInheritedFlags.overflowX = static_cast<unsigned>(initialOverflowX());
    mNonInheritedFlags.overflowY = static_cast<unsigned>(initialOverflowY());
    mNonInheritedFlags.position = static_cast<unsigned>(initialPosition());
}

const ComputedStyle& ComputedStyle::initialStyle() {
    static const ComputedStyle style;
    return style;
}

BuilderContext::BuilderContext(const ComputedStyle& style, const ComputedStyle* parentStyle, const ComputedStyle& rootStyle)
    : style(style)
    , parentStyle(parentStyle)
    , rootStyle(rootStyle) {}

BuilderState::BuilderState(ComputedStyle& style, const ComputedStyle* parentStyle, const ComputedStyle& rootStyle)
    : style(style)
    , parentStyle(parentStyle)
    , context(style, parentStyle, rootStyle) {}

BorderRadius::BorderRadius(CornerRadius topLeft, CornerRadius topRight, CornerRadius bottomRight, CornerRadius bottomLeft)
    : topLeft(topLeft)
    , topRight(topRight)
    , bottomRight(bottomRight)
    , bottomLeft(bottomLeft) {}

FlexibleBoxData::FlexibleBoxData()
    : flexGrow(ComputedStyle::initialFlexGrow())
    , flexShrink(ComputedStyle::initialFlexShrink())
    , flexBasis(ComputedStyle::initialFlexBasis())
    , flexDirection(static_cast<unsigned>(ComputedStyle::initialFlexDirection()))
    , flexWrap(ComputedStyle::initialFlexWrap().toRaw()) {}

FlexibleBoxData::FlexibleBoxData(const FlexibleBoxData& other)
    : flexGrow(other.flexGrow)
    , flexShrink(other.flexShrink)
    , flexBasis(other.flexBasis)
    , flexDirection(other.flexDirection)
    , flexWrap(other.flexWrap) {}

Ref<FlexibleBoxData> FlexibleBoxData::copy() const { return adoptRef(*new FlexibleBoxData(*this)); }

BoxData::BoxData()
    : width(ComputedStyle::initialWidth())
    , height(ComputedStyle::initialHeight())
    , minWidth(ComputedStyle::initialMinWidth())
    , minHeight(ComputedStyle::initialMinHeight())
    , maxWidth(ComputedStyle::initialMaxWidth())
    , maxHeight(ComputedStyle::initialMaxHeight())
    , verticalAlign(ComputedStyle::initialVerticalAlign()) {}

InheritedData::InheritedData()
    : fontData(FontData::create())
    , color(ComputedStyle::initialColor())
    , lineHeight(ComputedStyle::initialLineHeight()) {}

InheritedData::InheritedData(const InheritedData& other)
    : fontData(other.fontData)
    , color(other.color)
    , lineHeight(other.lineHeight) {}

Ref<InheritedData> InheritedData::copy() const { return adoptRef(*new InheritedData(*this)); }

FontDescription::FontDescription() {
    setFontSize(ComputedStyle::initialFontSize());
    setFontStyle(ComputedStyle::initialFontStyle());
    setFontWeight(ComputedStyle::initialFontWeight());
    setFontWidth(ComputedStyle::initialFontWidth());
}

FontCascadeDescription::FontCascadeDescription()
    : mFamilies(ComputedStyle::initialFontFamily()) {}

float FontDescription::fontSize() const { return mFontSize; }

void FontDescription::setFontSize(float value) { mFontSize = value; }

void FontDescription::setFontSize(CSS::Keyword::Medium) { setFontSize(kCssMediumSize); }

FontStyle FontDescription::fontStyle() const { return mSelection.style; }

void FontDescription::setFontStyle(FontStyle value) { mSelection.style = value; }

FontWeight FontDescription::fontWeight() const { return mSelection.weight; }

void FontDescription::setFontWeight(FontWeight value) { mSelection.weight = value; }

void FontDescription::setFontWeight(CSS::Keyword::Normal) { setFontWeight(FontWeight {400.f}); }

FontWidth FontDescription::fontWidth() const { return mSelection.width; }

void FontDescription::setFontWidth(FontWidth value) { mSelection.width = value; }

void FontDescription::setFontWidth(CSS::Keyword::Normal) { setFontWidth(FontWidth {100.f}); }

const FontFamilies& FontCascadeDescription::families() const { return mFamilies; }

void FontCascadeDescription::setFamilies(FontFamilies value) { mFamilies = std::move(value); }

FontData::FontData()
    : letterSpacing(ComputedStyle::initialLetterSpacing())
    , wordSpacing(ComputedStyle::initialWordSpacing()) {}

FontData::FontData(const FontData& other)
    : fontCascade(other.fontCascade)
    , letterSpacing(other.letterSpacing)
    , wordSpacing(other.wordSpacing) {}

Ref<FontData> FontData::copy() const { return adoptRef(*new FontData(*this)); }

bool FontData::operator==(const FontData& other) const {
    return fontCascade == other.fontCascade && letterSpacing == other.letterSpacing && wordSpacing == other.wordSpacing;
}

InheritedRareData::InheritedRareData()
    : accentColor(ComputedStyle::initialAccentColor())
    , colorScheme(ComputedStyle::initialColorScheme())
    , scrollbarColor(ComputedStyle::initialScrollbarColor()) {}

InheritedRareData::InheritedRareData(const InheritedRareData& other)
    : accentColor(other.accentColor)
    , colorScheme(other.colorScheme)
    , scrollbarColor(other.scrollbarColor) {}

Ref<InheritedRareData> InheritedRareData::copy() const { return adoptRef(*new InheritedRareData(*this)); }

NonInheritedMiscData::NonInheritedMiscData()
    : alignContent(ComputedStyle::initialAlignContent())
    , alignItems(ComputedStyle::initialAlignItems())
    , alignSelf(ComputedStyle::initialAlignSelf())
    , justifyContent(ComputedStyle::initialJustifyContent())
    , justifyItems(ComputedStyle::initialJustifyItems())
    , justifySelf(ComputedStyle::initialJustifySelf())
    , flexibleBox(FlexibleBoxData::create())
    , opacity(ComputedStyle::initialOpacity())
    , order(ComputedStyle::initialOrder())
    , rowGap(ComputedStyle::initialRowGap())
    , columnGap(ComputedStyle::initialColumnGap())
    , translate(ComputedStyle::initialTranslate())
    , shadows(ComputedStyle::initialBoxShadow())
    , textDecoration(static_cast<unsigned>(ComputedStyle::initialTextDecoration()))
    , textOverflow(static_cast<unsigned>(ComputedStyle::initialTextOverflow()))
    , appearance(static_cast<unsigned>(ComputedStyle::initialAppearance())) {}

NonInheritedMiscData::NonInheritedMiscData(const NonInheritedMiscData& other)
    : alignContent(other.alignContent)
    , alignItems(other.alignItems)
    , alignSelf(other.alignSelf)
    , justifyContent(other.justifyContent)
    , justifyItems(other.justifyItems)
    , justifySelf(other.justifySelf)
    , flexibleBox(other.flexibleBox)
    , opacity(other.opacity)
    , order(other.order)
    , rowGap(other.rowGap)
    , columnGap(other.columnGap)
    , translate(other.translate)
    , shadows(other.shadows)
    , textDecoration(other.textDecoration)
    , textOverflow(other.textOverflow)
    , appearance(other.appearance) {}

Ref<NonInheritedMiscData> NonInheritedMiscData::copy() const { return adoptRef(*new NonInheritedMiscData(*this)); }

NonInheritedRareData::NonInheritedRareData()
    : backdropFilter(ComputedStyle::initialBackdropFilter())
    , scrollbarGutter(ComputedStyle::initialScrollbarGutter())
    , scrollbarWidth(static_cast<unsigned>(ComputedStyle::initialScrollbarWidth())) {}

NonInheritedRareData::NonInheritedRareData(const NonInheritedRareData& other)
    : backdropFilter(other.backdropFilter)
    , scrollbarGutter(other.scrollbarGutter)
    , scrollbarWidth(other.scrollbarWidth) {}

Ref<NonInheritedRareData> NonInheritedRareData::copy() const { return adoptRef(*new NonInheritedRareData(*this)); }

BackgroundData::BackgroundData()
    : backgroundColor(ComputedStyle::initialBackgroundColor())
    , outline {.offset = ComputedStyle::initialOutlineOffset()} {}

BackgroundData::BackgroundData(const BackgroundData& other)
    : backgroundColor(other.backgroundColor)
    , outline(other.outline) {}

Ref<BackgroundData> BackgroundData::copy() const { return adoptRef(*new BackgroundData(*this)); }

BorderData::BorderData()
    : widths {ComputedStyle::initialBorderTopWidth(), ComputedStyle::initialBorderRightWidth(), ComputedStyle::initialBorderBottomWidth(),
          ComputedStyle::initialBorderLeftWidth()}
    , styles {static_cast<unsigned>(ComputedStyle::initialBorderTopStyle()),
          static_cast<unsigned>(ComputedStyle::initialBorderRightStyle()), static_cast<unsigned>(ComputedStyle::initialBorderBottomStyle()),
          static_cast<unsigned>(ComputedStyle::initialBorderLeftStyle())}
    , colors {ComputedStyle::initialBorderTopColor(), ComputedStyle::initialBorderRightColor(), ComputedStyle::initialBorderBottomColor(),
          ComputedStyle::initialBorderLeftColor()}
    , radii()
    , borderImage {ComputedStyle::initialBorderImageSource(), ComputedStyle::initialBorderImageSlice(),
          ComputedStyle::initialBorderImageWidth(), ComputedStyle::initialBorderImageOutset(), ComputedStyle::initialBorderImageRepeat()} {}

SurroundData::SurroundData()
    : border()
    , margin {ComputedStyle::initialMarginTop(), ComputedStyle::initialMarginRight(), ComputedStyle::initialMarginBottom(),
          ComputedStyle::initialMarginLeft()}
    , padding {ComputedStyle::initialPaddingTop(), ComputedStyle::initialPaddingRight(), ComputedStyle::initialPaddingBottom(),
          ComputedStyle::initialPaddingLeft()}
    , inset {ComputedStyle::initialTop(), ComputedStyle::initialRight(), ComputedStyle::initialBottom(), ComputedStyle::initialLeft()} {}

SurroundData::SurroundData(const SurroundData& other)
    : border(other.border)
    , margin(other.margin)
    , padding(other.padding)
    , inset(other.inset) {}

Ref<SurroundData> SurroundData::copy() const { return adoptRef(*new SurroundData(*this)); }

NonInheritedData::NonInheritedData()
    : backgroundData(BackgroundData::create())
    , surroundData(SurroundData::create())
    , miscData(NonInheritedMiscData::create())
    , rareData(NonInheritedRareData::create())
    , boxData() {}

NonInheritedData::NonInheritedData(const NonInheritedData& other)
    : backgroundData(other.backgroundData)
    , surroundData(other.surroundData)
    , miscData(other.miscData)
    , rareData(other.rareData)
    , boxData(other.boxData) {}

Ref<NonInheritedData> NonInheritedData::copy() const { return adoptRef(*new NonInheritedData(*this)); }
} // namespace Core::Style
