/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "style/computedstyle.h"
#include <algorithm>
#include <array>
#include <cstdint>
#include "ComputedStyleProperties.h"

namespace radia::ui {
namespace {
struct SystemColor {
    CSSKeyword keyword;
    std::uint32_t light;
    std::uint32_t dark;
};

const std::array kSystemColors{
    SystemColor{CSSKeyword::AccentColor, 0x0067c0, 0x8ab4f8},   SystemColor{CSSKeyword::AccentColorText, 0xffffff, 0x202124},
    SystemColor{CSSKeyword::ActiveText, 0xee0000, 0xf28b82},    SystemColor{CSSKeyword::ButtonBorder, 0x767676, 0x5f6368},
    SystemColor{CSSKeyword::ButtonFace, 0xf0f0f0, 0x3c4043},    SystemColor{CSSKeyword::ButtonText, 0x000000, 0xe8eaed},
    SystemColor{CSSKeyword::Canvas, 0xffffff, 0x1c1c1c},        SystemColor{CSSKeyword::CanvasText, 0x000000, 0xf5f5f5},
    SystemColor{CSSKeyword::Field, 0xffffff, 0x303134},         SystemColor{CSSKeyword::FieldText, 0x000000, 0xe8eaed},
    SystemColor{CSSKeyword::GrayText, 0x6d6d6d, 0x9aa0a6},      SystemColor{CSSKeyword::Highlight, 0xb4d5ff, 0x2b65a8},
    SystemColor{CSSKeyword::HighlightText, 0x000000, 0xffffff}, SystemColor{CSSKeyword::LinkText, 0x0000ee, 0x8ab4f8},
    SystemColor{CSSKeyword::Mark, 0xffff00, 0x5c4a00},          SystemColor{CSSKeyword::MarkText, 0x000000, 0xffeb3b},
    SystemColor{CSSKeyword::SelectedItem, 0x3875d7, 0x174ea6},  SystemColor{CSSKeyword::SelectedItemText, 0xffffff, 0xffffff},
    SystemColor{CSSKeyword::VisitedText, 0x551a8b, 0xc58af9},
};
} // namespace

Color systemColorValue(CSSKeyword keyword, ColorSchemeMode scheme) {
    const auto found =
        std::find_if(kSystemColors.begin(), kSystemColors.end(), [keyword](const SystemColor& color) { return color.keyword == keyword; });
    if (found == kSystemColors.end()) return Color(0.f, 0.f, 0.f);
    const std::uint32_t rgb = scheme == ColorSchemeMode::Light ? found->light : found->dark;
    constexpr float scale = 1.f / 255.f;
    return Color{((rgb >> 16) & 0xff) * scale, ((rgb >> 8) & 0xff) * scale, (rgb & 0xff) * scale};
}

StyleColor StyleColor::current() {
    return StyleColor(CSSKeyword::CurrentColor);
}

StyleColor StyleColor::fromKeyword(CSSKeyword keyword) {
    if (keyword == CSSKeyword::Transparent) return fromColor(Color(0.f, 0.f, 0.f, 0.f));
    if (keyword == CSSKeyword::CurrentColor || isSystemColorKeyword(keyword)) return StyleColor(keyword);
    return fromColor(Color(0.f, 0.f, 0.f));
}

bool StyleColor::isCurrentColor() const {
    const auto* keyword = std::get_if<CSSKeyword>(&mValue);
    return keyword && *keyword == CSSKeyword::CurrentColor;
}

bool StyleColor::isSystemColor() const {
    const auto* keyword = std::get_if<CSSKeyword>(&mValue);
    return keyword && isSystemColorKeyword(*keyword);
}

std::optional<StyleColor> StyleColor::fromCSS(const CSS::Color& value, ColorSchemeMode scheme) {
    const CSS::Color* selected = &value;
    while (const CSS::LightDarkColor* lightDark = selected->lightDark())
        selected = scheme == ColorSchemeMode::Light ? &lightDark->light : &lightDark->dark;

    if (const Color* color = selected->solidColor()) return fromColor(*color);
    if (const CSS::Keyword* keyword = selected->keyword()) {
        if (keyword->id == CSSKeyword::CurrentColor) return StyleColor(keyword->id);
        if (isSystemColorKeyword(keyword->id)) return fromColor(systemColorValue(keyword->id, scheme));
    }
    return std::nullopt;
}

void StyleColor::resolve(const Color& currentColorValue, ColorSchemeMode scheme) {
    const auto* keyword = std::get_if<CSSKeyword>(&mValue);
    if (!keyword) return;
    if (*keyword == CSSKeyword::CurrentColor) mValue = currentColorValue;
    else if (isSystemColorKeyword(*keyword)) mValue = systemColorValue(*keyword, scheme);
}

ComputedStyle::ComputedStyle()
    : mInheritedData(InheritedData::create()), mInheritedRareData(InheritedRareData::create()), mNonInheritedData(NonInheritedData::create()) {
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

StyleBuilderContext::StyleBuilderContext(const ComputedStyle& style, const ComputedStyle* parentStyle, const ComputedStyle& rootStyle)
    : style(style), parentStyle(parentStyle), rootStyle(rootStyle) {}

StyleBuilderState::StyleBuilderState(ComputedStyle& style, const ComputedStyle* parentStyle, const ComputedStyle& rootStyle)
    : style(style), parentStyle(parentStyle), context(style, parentStyle, rootStyle) {}

BorderRadius::BorderRadius(CornerRadius topLeft, CornerRadius topRight, CornerRadius bottomRight, CornerRadius bottomLeft)
    : topLeft(topLeft), topRight(topRight), bottomRight(bottomRight), bottomLeft(bottomLeft) {}

FlexibleBoxData::FlexibleBoxData()
    : flexGrow(ComputedStyle::initialFlexGrow()), flexShrink(ComputedStyle::initialFlexShrink()), flexBasis(ComputedStyle::initialFlexBasis()),
      flexDirection(static_cast<unsigned>(ComputedStyle::initialFlexDirection())), flexWrap(ComputedStyle::initialFlexWrap().toRaw()) {}

FlexibleBoxData::FlexibleBoxData(const FlexibleBoxData& other)
    : flexGrow(other.flexGrow), flexShrink(other.flexShrink), flexBasis(other.flexBasis), flexDirection(other.flexDirection),
      flexWrap(other.flexWrap) {}

Ref<FlexibleBoxData> FlexibleBoxData::copy() const {
    return adoptRef(*new FlexibleBoxData(*this));
}

BoxData::BoxData()
    : width(ComputedStyle::initialWidth()), height(ComputedStyle::initialHeight()), minWidth(ComputedStyle::initialMinWidth()),
      minHeight(ComputedStyle::initialMinHeight()), maxWidth(ComputedStyle::initialMaxWidth()), maxHeight(ComputedStyle::initialMaxHeight()),
      verticalAlign(ComputedStyle::initialVerticalAlign()) {}

InheritedData::InheritedData() : fontData(FontData::create()), color(ComputedStyle::initialColor()), lineHeight(ComputedStyle::initialLineHeight()) {}

InheritedData::InheritedData(const InheritedData& other) : fontData(other.fontData), color(other.color), lineHeight(other.lineHeight) {}

Ref<InheritedData> InheritedData::copy() const {
    return adoptRef(*new InheritedData(*this));
}

FontDescription::FontDescription() {
    setFontSize(ComputedStyle::initialFontSize());
    setFontStyle(ComputedStyle::initialFontStyle());
    setFontWeight(ComputedStyle::initialFontWeight());
    setFontWidth(ComputedStyle::initialFontWidth());
}

FontCascadeDescription::FontCascadeDescription() : mFamilies(ComputedStyle::initialFontFamily()) {}

float FontDescription::fontSize() const {
    return mFontSize;
}

void FontDescription::setFontSize(float value) {
    mFontSize = value;
}

void FontDescription::setFontSize(CSS::Keyword::Medium) {
    setFontSize(cssMediumSize);
}

FontStyle FontDescription::fontStyle() const {
    return mSelection.style;
}

void FontDescription::setFontStyle(FontStyle value) {
    mSelection.style = value;
}

FontWeight FontDescription::fontWeight() const {
    return mSelection.weight;
}

void FontDescription::setFontWeight(FontWeight value) {
    mSelection.weight = value;
}

void FontDescription::setFontWeight(CSS::Keyword::Normal) {
    setFontWeight(FontWeight{400.f});
}

FontWidth FontDescription::fontWidth() const {
    return mSelection.width;
}

void FontDescription::setFontWidth(FontWidth value) {
    mSelection.width = value;
}

void FontDescription::setFontWidth(CSS::Keyword::Normal) {
    setFontWidth(FontWidth{100.f});
}

const FontFamilies& FontCascadeDescription::families() const {
    return mFamilies;
}

void FontCascadeDescription::setFamilies(FontFamilies value) {
    mFamilies = std::move(value);
}

FontData::FontData() : letterSpacing(ComputedStyle::initialLetterSpacing()), wordSpacing(ComputedStyle::initialWordSpacing()) {}

FontData::FontData(const FontData& other) : fontCascade(other.fontCascade), letterSpacing(other.letterSpacing), wordSpacing(other.wordSpacing) {}

Ref<FontData> FontData::copy() const {
    return adoptRef(*new FontData(*this));
}

bool FontData::operator==(const FontData& other) const {
    return fontCascade == other.fontCascade && letterSpacing == other.letterSpacing && wordSpacing == other.wordSpacing;
}

InheritedRareData::InheritedRareData()
    : accentColor(ComputedStyle::initialAccentColor()), colorScheme(ComputedStyle::initialColorScheme()),
      scrollbarColor(ComputedStyle::initialScrollbarColor()) {}

InheritedRareData::InheritedRareData(const InheritedRareData& other)
    : accentColor(other.accentColor), colorScheme(other.colorScheme), scrollbarColor(other.scrollbarColor) {}

Ref<InheritedRareData> InheritedRareData::copy() const {
    return adoptRef(*new InheritedRareData(*this));
}

NonInheritedMiscData::NonInheritedMiscData()
    : alignContent(ComputedStyle::initialAlignContent()), alignItems(ComputedStyle::initialAlignItems()),
      alignSelf(ComputedStyle::initialAlignSelf()), justifyContent(ComputedStyle::initialJustifyContent()),
      justifyItems(ComputedStyle::initialJustifyItems()), justifySelf(ComputedStyle::initialJustifySelf()), flexibleBox(FlexibleBoxData::create()),
      opacity(ComputedStyle::initialOpacity()), order(ComputedStyle::initialOrder()), rowGap(ComputedStyle::initialRowGap()),
      columnGap(ComputedStyle::initialColumnGap()), translate(ComputedStyle::initialTranslate()), shadows(ComputedStyle::initialBoxShadow()),
      textDecoration(static_cast<unsigned>(ComputedStyle::initialTextDecoration())),
      textOverflow(static_cast<unsigned>(ComputedStyle::initialTextOverflow())),
      appearance(static_cast<unsigned>(ComputedStyle::initialAppearance())) {}

NonInheritedMiscData::NonInheritedMiscData(const NonInheritedMiscData& other)
    : alignContent(other.alignContent), alignItems(other.alignItems), alignSelf(other.alignSelf), justifyContent(other.justifyContent),
      justifyItems(other.justifyItems), justifySelf(other.justifySelf), flexibleBox(other.flexibleBox), opacity(other.opacity), order(other.order),
      rowGap(other.rowGap), columnGap(other.columnGap), translate(other.translate), shadows(other.shadows), textDecoration(other.textDecoration),
      textOverflow(other.textOverflow), appearance(other.appearance) {}

Ref<NonInheritedMiscData> NonInheritedMiscData::copy() const {
    return adoptRef(*new NonInheritedMiscData(*this));
}

NonInheritedRareData::NonInheritedRareData()
    : backdropFilter(ComputedStyle::initialBackdropFilter()), scrollbarGutter(ComputedStyle::initialScrollbarGutter()),
      scrollbarWidth(static_cast<unsigned>(ComputedStyle::initialScrollbarWidth())) {}

NonInheritedRareData::NonInheritedRareData(const NonInheritedRareData& other)
    : backdropFilter(other.backdropFilter), scrollbarGutter(other.scrollbarGutter), scrollbarWidth(other.scrollbarWidth) {}

Ref<NonInheritedRareData> NonInheritedRareData::copy() const {
    return adoptRef(*new NonInheritedRareData(*this));
}

BackgroundData::BackgroundData()
    : backgroundColor(ComputedStyle::initialBackgroundColor()), outline{.offset = ComputedStyle::initialOutlineOffset()} {}

BackgroundData::BackgroundData(const BackgroundData& other) : backgroundColor(other.backgroundColor), outline(other.outline) {}

Ref<BackgroundData> BackgroundData::copy() const {
    return adoptRef(*new BackgroundData(*this));
}

BorderData::BorderData()
    : widths{ComputedStyle::initialBorderTopWidth(), ComputedStyle::initialBorderRightWidth(), ComputedStyle::initialBorderBottomWidth(),
             ComputedStyle::initialBorderLeftWidth()},
      styles{static_cast<unsigned>(ComputedStyle::initialBorderTopStyle()), static_cast<unsigned>(ComputedStyle::initialBorderRightStyle()),
             static_cast<unsigned>(ComputedStyle::initialBorderBottomStyle()), static_cast<unsigned>(ComputedStyle::initialBorderLeftStyle())},
      colors{ComputedStyle::initialBorderTopColor(), ComputedStyle::initialBorderRightColor(), ComputedStyle::initialBorderBottomColor(),
             ComputedStyle::initialBorderLeftColor()},
      radii(),
      borderImage{ComputedStyle::initialBorderImageSource(), ComputedStyle::initialBorderImageSlice(), ComputedStyle::initialBorderImageWidth(),
                  ComputedStyle::initialBorderImageOutset(), ComputedStyle::initialBorderImageRepeat()} {}

SurroundData::SurroundData()
    : border(), margin{ComputedStyle::initialMarginTop(), ComputedStyle::initialMarginRight(), ComputedStyle::initialMarginBottom(),
                       ComputedStyle::initialMarginLeft()},
      padding{ComputedStyle::initialPaddingTop(), ComputedStyle::initialPaddingRight(), ComputedStyle::initialPaddingBottom(),
              ComputedStyle::initialPaddingLeft()},
      inset{ComputedStyle::initialTop(), ComputedStyle::initialRight(), ComputedStyle::initialBottom(), ComputedStyle::initialLeft()} {}

SurroundData::SurroundData(const SurroundData& other) : border(other.border), margin(other.margin), padding(other.padding), inset(other.inset) {}

Ref<SurroundData> SurroundData::copy() const {
    return adoptRef(*new SurroundData(*this));
}

NonInheritedData::NonInheritedData()
    : backgroundData(BackgroundData::create()), surroundData(SurroundData::create()), miscData(NonInheritedMiscData::create()),
      rareData(NonInheritedRareData::create()), boxData() {}

NonInheritedData::NonInheritedData(const NonInheritedData& other)
    : backgroundData(other.backgroundData), surroundData(other.surroundData), miscData(other.miscData), rareData(other.rareData),
      boxData(other.boxData) {}

Ref<NonInheritedData> NonInheritedData::copy() const {
    return adoptRef(*new NonInheritedData(*this));
}
} // namespace radia::ui
