/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include <algorithm>
#include <array>
#include <cctype>
#include <cmath>
#include <ranges>
#include <fontconfig/fontconfig.h>
#include "SystemFontProvider.h"

namespace Core::detail {
namespace {
bool initializeFontconfig() {
    static const bool initialized = FcInit();
    return initialized;
}

std::string lowerASCII(std::string_view value) {
    std::string result(value);
    std::ranges::transform(result, result.begin(), [](unsigned char ch) {
        return static_cast<char>(std::tolower(ch));
    });
    return result;
}

const char* genericName(Style::GenericFontFamily family) {
    switch (family) {
    case Style::GenericFontFamily::Serif:
        return "serif";
    case Style::GenericFontFamily::SansSerif:
        return "sans-serif";
    case Style::GenericFontFamily::SystemUI:
        return "system-ui";
    case Style::GenericFontFamily::UISansSerif:
        return "ui-sans-serif";
    case Style::GenericFontFamily::UIRounded:
        return "ui-rounded";
    case Style::GenericFontFamily::Cursive:
        return "cursive";
    case Style::GenericFontFamily::Fantasy:
        return "fantasy";
    case Style::GenericFontFamily::Math:
        return "math";
    case Style::GenericFontFamily::Monospace:
        return "monospace";
    case Style::GenericFontFamily::UIMonospace:
        return "ui-monospace";
    case Style::GenericFontFamily::UISerif:
        return "ui-serif";
    case Style::GenericFontFamily::Fangsong:
        return "fangsong";
    case Style::GenericFontFamily::Kai:
        return "KaiTi";
    case Style::GenericFontFamily::KhmerMul:
        return "Khmer OS Muol";
    case Style::GenericFontFamily::Nastaliq:
        return "Noto Nastaliq Urdu";
    }
    return "sans-serif";
}

int fontconfigWidth(float percentage) {
    constexpr std::array<float, 9> widths {50.f, 62.5f, 75.f, 87.5f, 100.f, 112.5f, 125.f, 150.f, 200.f};
    constexpr std::array<int, 9> values {FC_WIDTH_ULTRACONDENSED, FC_WIDTH_EXTRACONDENSED, FC_WIDTH_CONDENSED, FC_WIDTH_SEMICONDENSED,
        FC_WIDTH_NORMAL, FC_WIDTH_SEMIEXPANDED, FC_WIDTH_EXPANDED, FC_WIDTH_EXTRAEXPANDED, FC_WIDTH_ULTRAEXPANDED};
    const auto closest = std::min_element(widths.begin(), widths.end(), [percentage](float a, float b) {
        return std::abs(a - percentage) < std::abs(b - percentage);
    });
    return values[static_cast<std::size_t>(closest - widths.begin())];
}

void addRequest(FcPattern* pattern, const Style::FontSelectionRequest& request) {
    FcPatternAddInteger(pattern, FC_WEIGHT, FcWeightFromOpenType(static_cast<int>(request.weight.value)));
    FcPatternAddInteger(pattern, FC_WIDTH, fontconfigWidth(request.width.percentage));
    FcPatternAddInteger(pattern, FC_SLANT,
        request.style == Style::FontStyle::Italic        ? FC_SLANT_ITALIC
            : request.style == Style::FontStyle::Oblique ? FC_SLANT_OBLIQUE
                                                         : FC_SLANT_ROMAN);
}

bool hasExactFamily(std::string_view familyName) {
    FcPattern* pattern = FcPatternCreate();
    FcObjectSet* objects = FcObjectSetBuild(FC_FAMILY, nullptr);
    if (!pattern || !objects) {
        if (pattern)
            FcPatternDestroy(pattern);
        if (objects)
            FcObjectSetDestroy(objects);
        return false;
    }
    const std::string family(familyName);
    FcPatternAddString(pattern, FC_FAMILY, reinterpret_cast<const FcChar8*>(family.c_str()));
    FcFontSet* fonts = FcFontList(nullptr, pattern, objects);
    FcPatternDestroy(pattern);
    FcObjectSetDestroy(objects);
    if (!fonts)
        return false;

    const std::string expected = lowerASCII(familyName);
    bool found = false;
    for (int index = 0; index < fonts->nfont && !found; ++index) {
        FcChar8* name = nullptr;
        for (int familyIndex = 0; FcPatternGetString(fonts->fonts[index], FC_FAMILY, familyIndex, &name) == FcResultMatch; ++familyIndex) {
            if (lowerASCII(reinterpret_cast<const char*>(name)) == expected) {
                found = true;
                break;
            }
        }
    }
    FcFontSetDestroy(fonts);
    return found;
}

std::optional<SystemFontMatch> match(std::string_view familyName, const Style::FontSelectionRequest& request, bool requireFamily) {
    if (familyName.empty() || (requireFamily && !hasExactFamily(familyName)))
        return std::nullopt;

    FcPattern* pattern = FcPatternCreate();
    if (!pattern)
        return std::nullopt;
    const std::string family(familyName);
    FcPatternAddString(pattern, FC_FAMILY, reinterpret_cast<const FcChar8*>(family.c_str()));
    addRequest(pattern, request);
    FcConfigSubstitute(nullptr, pattern, FcMatchPattern);
    FcDefaultSubstitute(pattern);

    FcResult result = FcResultNoMatch;
    FcPattern* font = FcFontMatch(nullptr, pattern, &result);
    FcPatternDestroy(pattern);
    if (!font || result != FcResultMatch) {
        if (font)
            FcPatternDestroy(font);
        return std::nullopt;
    }

    FcChar8* path = nullptr;
    int faceIndex = 0;
    const bool hasPath = FcPatternGetString(font, FC_FILE, 0, &path) == FcResultMatch;
    FcPatternGetInteger(font, FC_INDEX, 0, &faceIndex);
    FcChar8* matchedFamilyName = nullptr;
    FcChar8* fullName = nullptr;
    FcChar8* postScriptName = nullptr;
    FcPatternGetString(font, FC_FAMILY, 0, &matchedFamilyName);
    FcPatternGetString(font, FC_FULLNAME, 0, &fullName);
    FcPatternGetString(font, FC_POSTSCRIPT_NAME, 0, &postScriptName);
    std::optional<SystemFontMatch> match;
    if (hasPath && path)
        match.emplace(SystemFontMatch {reinterpret_cast<const char*>(path), faceIndex,
            matchedFamilyName ? std::string(reinterpret_cast<const char*>(matchedFamilyName)) : std::string {},
            fullName ? std::string(reinterpret_cast<const char*>(fullName)) : std::string {},
            postScriptName ? std::string(reinterpret_cast<const char*>(postScriptName)) : std::string {}});
    FcPatternDestroy(font);
    return match;
}

bool exactName(FcPattern* font, const char* propertyName, std::string_view expected) {
    const std::string folded = lowerASCII(expected);
    FcChar8* value = nullptr;
    for (int index = 0; FcPatternGetString(font, propertyName, index, &value) == FcResultMatch; ++index)
        if (lowerASCII(reinterpret_cast<const char*>(value)) == folded)
            return true;
    return false;
}
} // namespace

std::optional<SystemFontMatch> matchSystemFont(const Style::FontFamily& family, const Style::FontSelectionRequest& request) {
    if (!initializeFontconfig())
        return std::nullopt;
    if (const auto* name = std::get_if<std::string>(&family))
        return match(*name, request, true);
    return match(genericName(std::get<Style::GenericFontFamily>(family)), request, false);
}

std::optional<SystemFontMatch> matchLocalFont(std::string_view name) {
    if (name.empty() || !initializeFontconfig())
        return std::nullopt;
    FcPattern* pattern = FcPatternCreate();
    FcObjectSet* objects = FcObjectSetBuild(FC_FAMILY, FC_FULLNAME, FC_POSTSCRIPT_NAME, FC_FILE, FC_INDEX, nullptr);
    if (!pattern || !objects) {
        if (pattern)
            FcPatternDestroy(pattern);
        if (objects)
            FcObjectSetDestroy(objects);
        return std::nullopt;
    }
    FcFontSet* fonts = FcFontList(nullptr, pattern, objects);
    FcPatternDestroy(pattern);
    FcObjectSetDestroy(objects);
    if (!fonts)
        return std::nullopt;

    const std::string expected = lowerASCII(name);
    std::optional<SystemFontMatch> result;
    for (int index = 0; index < fonts->nfont; ++index) {
        FcPattern* font = fonts->fonts[index];
        if (!exactName(font, FC_FULLNAME, expected) && !exactName(font, FC_POSTSCRIPT_NAME, expected))
            continue;
        FcChar8* path = nullptr;
        int faceIndex = 0;
        if (FcPatternGetString(font, FC_FILE, 0, &path) == FcResultMatch && path) {
            FcPatternGetInteger(font, FC_INDEX, 0, &faceIndex);
            FcChar8* familyName = nullptr;
            FcChar8* fullName = nullptr;
            FcChar8* postScriptName = nullptr;
            FcPatternGetString(font, FC_FAMILY, 0, &familyName);
            FcPatternGetString(font, FC_FULLNAME, 0, &fullName);
            FcPatternGetString(font, FC_POSTSCRIPT_NAME, 0, &postScriptName);
            result.emplace(SystemFontMatch {reinterpret_cast<const char*>(path), faceIndex,
                familyName ? std::string(reinterpret_cast<const char*>(familyName)) : std::string {},
                fullName ? std::string(reinterpret_cast<const char*>(fullName)) : std::string {},
                postScriptName ? std::string(reinterpret_cast<const char*>(postScriptName)) : std::string {}});
            break;
        }
    }
    FcFontSetDestroy(fonts);
    return result;
}
} // namespace Core::detail
