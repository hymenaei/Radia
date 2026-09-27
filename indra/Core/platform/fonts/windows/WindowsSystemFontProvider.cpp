/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <array>
#include <cmath>
#include <dwrite.h>
#include <limits>
#include <span>
#include <string>
#include <string_view>
#include <Windows.h>
#include <wrl/client.h>
#include "platform/fonts/SystemFontProvider.h"

namespace radia::ui::detail {
namespace {
using Microsoft::WRL::ComPtr;

std::optional<std::wstring> toWide(std::string_view value) {
    if (value.size() > static_cast<std::size_t>(std::numeric_limits<int>::max())) return std::nullopt;
    if (value.empty()) return std::wstring{};
    const int length = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), nullptr, 0);
    if (length <= 0) return std::nullopt;

    std::wstring result(static_cast<std::size_t>(length), L'\0');
    if (MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), result.data(), length) != length)
        return std::nullopt;
    return result;
}

std::optional<std::string> toUtf8(std::wstring_view value) {
    if (value.size() > static_cast<std::size_t>(std::numeric_limits<int>::max())) return std::nullopt;
    if (value.empty()) return std::string{};

    const int length = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), nullptr, 0, nullptr, nullptr);
    if (length <= 0) return std::nullopt;

    std::string result(static_cast<std::size_t>(length), '\0');
    if (WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS, value.data(), static_cast<int>(value.size()), result.data(), length, nullptr, nullptr)
        != length)
        return std::nullopt;
    return result;
}

bool equalIgnoreCase(std::wstring_view left, std::wstring_view right) {
    if (left.size() > static_cast<std::size_t>(std::numeric_limits<int>::max())
        || right.size() > static_cast<std::size_t>(std::numeric_limits<int>::max()))
        return false;
    return CompareStringOrdinal(left.data(), static_cast<int>(left.size()), right.data(), static_cast<int>(right.size()), TRUE) == CSTR_EQUAL;
}

ComPtr<IDWriteFactory> makeFactory() {
    ComPtr<IUnknown> unknown;
    if (FAILED(DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED, __uuidof(IDWriteFactory), unknown.GetAddressOf()))) return {};

    ComPtr<IDWriteFactory> factory;
    if (FAILED(unknown.As(&factory))) return {};
    return factory;
}

ComPtr<IDWriteFontCollection> getSystemFontCollection(IDWriteFactory* factory) {
    ComPtr<IDWriteFontCollection> collection;
    if (FAILED(factory->GetSystemFontCollection(&collection, FALSE))) return {};
    return collection;
}

ComPtr<IDWriteLocalizedStrings> getInformationalStrings(IDWriteFont* font, DWRITE_INFORMATIONAL_STRING_ID id) {
    ComPtr<IDWriteLocalizedStrings> strings;
    BOOL exists = FALSE;
    if (FAILED(font->GetInformationalStrings(id, &strings, &exists)) || !exists || !strings) return {};
    return strings;
}

std::optional<std::wstring> stringAt(IDWriteLocalizedStrings* strings, UINT32 index) {
    UINT32 length = 0;
    if (FAILED(strings->GetStringLength(index, &length)) || length == std::numeric_limits<UINT32>::max()) return std::nullopt;

    std::wstring value(static_cast<std::size_t>(length) + 1, L'\0');
    if (FAILED(strings->GetString(index, value.data(), length + 1))) return std::nullopt;
    value.resize(length);
    return value;
}

bool containsName(IDWriteLocalizedStrings* strings, std::wstring_view expected) {
    for (UINT32 index = 0; index < strings->GetCount(); ++index)
        if (const auto value = stringAt(strings, index); value && equalIgnoreCase(*value, expected)) return true;
    return false;
}

std::optional<std::wstring> firstName(IDWriteLocalizedStrings* strings) {
    if (!strings) return std::nullopt;
    for (UINT32 index = 0; index < strings->GetCount(); ++index)
        if (const auto value = stringAt(strings, index); value && !value->empty()) return value;
    return std::nullopt;
}

ComPtr<IDWriteFontFamily> findFamily(IDWriteFontCollection* collection, std::wstring_view expected) {
    for (UINT32 index = 0; index < collection->GetFontFamilyCount(); ++index) {
        ComPtr<IDWriteFontFamily> family;
        if (FAILED(collection->GetFontFamily(index, &family))) continue;

        ComPtr<IDWriteLocalizedStrings> names;
        if (SUCCEEDED(family->GetFamilyNames(&names)) && names && containsName(names.Get(), expected)) return family;
    }
    return {};
}

DWRITE_FONT_WEIGHT toDWriteWeight(float weight) {
    if (!std::isfinite(weight)) weight = 400.f;
    const float bounded = std::clamp(weight, 1.f, 999.f);
    return static_cast<DWRITE_FONT_WEIGHT>(static_cast<int>(std::lround(bounded)));
}

DWRITE_FONT_STRETCH toDWriteStretch(float percentage) {
    constexpr std::array<float, 9> percentages{50.f, 62.5f, 75.f, 87.5f, 100.f, 112.5f, 125.f, 150.f, 200.f};
    if (!std::isfinite(percentage)) percentage = 100.f;

    const auto closest = std::min_element(percentages.begin(), percentages.end(), [percentage](float left, float right) {
        return std::abs(left - percentage) < std::abs(right - percentage);
    });
    return static_cast<DWRITE_FONT_STRETCH>(std::distance(percentages.begin(), closest) + 1);
}

DWRITE_FONT_STYLE toDWriteStyle(FontStyle style) {
    switch (style) {
        case FontStyle::Normal: return DWRITE_FONT_STYLE_NORMAL;
        case FontStyle::Italic: return DWRITE_FONT_STYLE_ITALIC;
        case FontStyle::Oblique: return DWRITE_FONT_STYLE_OBLIQUE;
    }
    return DWRITE_FONT_STYLE_NORMAL;
}

ComPtr<IDWriteFont> matchingFont(IDWriteFontFamily* family, const FontSelectionRequest& request) {
    ComPtr<IDWriteFont> font;
    if (FAILED(family->GetFirstMatchingFont(toDWriteWeight(request.weight.value), toDWriteStretch(request.width.percentage),
                                            toDWriteStyle(request.style), &font)))
        return {};
    return font;
}

std::optional<std::string> localFontPath(IDWriteFontFace* face) {
    UINT32 fileCount = 0;
    if (FAILED(face->GetFiles(&fileCount, nullptr)) || fileCount != 1) return std::nullopt;
    fileCount = 1;

    IDWriteFontFile* rawFile = nullptr;
    const HRESULT fileResult = face->GetFiles(&fileCount, &rawFile);
    ComPtr<IDWriteFontFile> file;
    file.Attach(rawFile);
    if (FAILED(fileResult) || !file) return std::nullopt;

    ComPtr<IDWriteFontFileLoader> loader;
    if (FAILED(file->GetLoader(&loader))) return std::nullopt;
    ComPtr<IDWriteLocalFontFileLoader> localLoader;
    if (FAILED(loader.As(&localLoader))) return std::nullopt;

    const void* referenceKey = nullptr;
    UINT32 referenceKeySize = 0;
    if (FAILED(file->GetReferenceKey(&referenceKey, &referenceKeySize))) return std::nullopt;

    UINT32 pathLength = 0;
    if (FAILED(localLoader->GetFilePathLengthFromKey(referenceKey, referenceKeySize, &pathLength))
        || pathLength == 0
        || pathLength == std::numeric_limits<UINT32>::max())
        return std::nullopt;

    std::wstring path(static_cast<std::size_t>(pathLength) + 1, L'\0');
    if (FAILED(localLoader->GetFilePathFromKey(referenceKey, referenceKeySize, path.data(), pathLength + 1))) return std::nullopt;
    path.resize(pathLength);
    return toUtf8(path);
}

std::optional<std::wstring> resolvedFamilyName(IDWriteFont* font) {
    ComPtr<IDWriteFontFamily> family;
    if (FAILED(font->GetFontFamily(&family))) return std::nullopt;

    ComPtr<IDWriteLocalizedStrings> names;
    if (FAILED(family->GetFamilyNames(&names)) || !names) return std::nullopt;
    return firstName(names.Get());
}

std::optional<SystemFontMatch> makeMatch(IDWriteFont* font) {
    ComPtr<IDWriteFontFace> face;
    if (FAILED(font->CreateFontFace(&face)) || face->GetIndex() > static_cast<UINT32>(std::numeric_limits<int>::max())) return std::nullopt;
    const auto path = localFontPath(face.Get());
    if (!path) return std::nullopt;

    const auto familyName = resolvedFamilyName(font);
    const auto fullName = firstName(getInformationalStrings(font, DWRITE_INFORMATIONAL_STRING_FULL_NAME).Get());
    const auto postScriptName = firstName(getInformationalStrings(font, DWRITE_INFORMATIONAL_STRING_POSTSCRIPT_NAME).Get());
    SystemFontMatch result;
    result.path = *path;
    result.faceIndex = static_cast<int>(face->GetIndex());
    if (familyName) result.familyName = toUtf8(*familyName).value_or(std::string{});
    if (fullName) result.fullName = toUtf8(*fullName).value_or(std::string{});
    if (postScriptName) result.postScriptName = toUtf8(*postScriptName).value_or(std::string{});
    return result;
}

std::span<const std::wstring_view> genericFamilies(GenericFontFamily family) {
    static constexpr std::array<std::wstring_view, 2> serif{L"Times New Roman", L"Georgia"};
    static constexpr std::array<std::wstring_view, 2> sansSerif{L"Arial", L"Segoe UI"};
    static constexpr std::array<std::wstring_view, 1> systemUI{L"Segoe UI"};
    static constexpr std::array<std::wstring_view, 2> uiSerif{L"Georgia", L"Times New Roman"};
    static constexpr std::array<std::wstring_view, 1> cursive{L"Comic Sans MS"};
    static constexpr std::array<std::wstring_view, 1> fantasy{L"Impact"};
    static constexpr std::array<std::wstring_view, 1> math{L"Cambria Math"};
    static constexpr std::array<std::wstring_view, 2> monospace{L"Consolas", L"Courier New"};
    static constexpr std::array<std::wstring_view, 2> uiMonospace{L"Cascadia Mono", L"Consolas"};
    static constexpr std::array<std::wstring_view, 2> uiRounded{L"Arial Rounded MT Bold", L"Segoe UI"};
    static constexpr std::array<std::wstring_view, 1> fangsong{L"FangSong"};
    static constexpr std::array<std::wstring_view, 1> kai{L"KaiTi"};
    static constexpr std::array<std::wstring_view, 2> khmerMul{L"Khmer OS Muol", L"Khmer UI"};
    static constexpr std::array<std::wstring_view, 2> nastaliq{L"Noto Nastaliq Urdu", L"Urdu Typesetting"};

    switch (family) {
        case GenericFontFamily::Serif: return serif;
        case GenericFontFamily::SansSerif: return sansSerif;
        case GenericFontFamily::SystemUI:
        case GenericFontFamily::UISansSerif: return systemUI;
        case GenericFontFamily::Cursive: return cursive;
        case GenericFontFamily::Fantasy: return fantasy;
        case GenericFontFamily::Math: return math;
        case GenericFontFamily::Monospace: return monospace;
        case GenericFontFamily::UISerif: return uiSerif;
        case GenericFontFamily::UIMonospace: return uiMonospace;
        case GenericFontFamily::UIRounded: return uiRounded;
        case GenericFontFamily::Fangsong: return fangsong;
        case GenericFontFamily::Kai: return kai;
        case GenericFontFamily::KhmerMul: return khmerMul;
        case GenericFontFamily::Nastaliq: return nastaliq;
    }
    return sansSerif;
}

std::optional<SystemFontMatch> matchFamily(IDWriteFontCollection* collection, std::wstring_view name, const FontSelectionRequest& request) {
    const auto family = findFamily(collection, name);
    if (!family) return std::nullopt;

    const auto font = matchingFont(family.Get(), request);
    if (!font) return std::nullopt;
    return makeMatch(font.Get());
}
} // namespace

std::optional<SystemFontMatch> matchSystemFont(const FontFamily& family, const FontSelectionRequest& request) {
    const auto factory = makeFactory();
    if (!factory) return std::nullopt;
    const auto collection = getSystemFontCollection(factory.Get());
    if (!collection) return std::nullopt;

    if (const auto* name = std::get_if<std::string>(&family)) {
        const auto wideName = toWide(*name);
        if (!wideName || wideName->empty()) return std::nullopt;
        return matchFamily(collection.Get(), *wideName, request);
    }

    for (const std::wstring_view name : genericFamilies(std::get<GenericFontFamily>(family)))
        if (const auto match = matchFamily(collection.Get(), name, request)) return match;
    return std::nullopt;
}

std::optional<SystemFontMatch> matchLocalFont(std::string_view name) {
    const auto wideName = toWide(name);
    if (!wideName || wideName->empty()) return std::nullopt;

    const auto factory = makeFactory();
    if (!factory) return std::nullopt;
    const auto collection = getSystemFontCollection(factory.Get());
    if (!collection) return std::nullopt;

    for (UINT32 familyIndex = 0; familyIndex < collection->GetFontFamilyCount(); ++familyIndex) {
        ComPtr<IDWriteFontFamily> family;
        if (FAILED(collection->GetFontFamily(familyIndex, &family))) continue;

        for (UINT32 fontIndex = 0; fontIndex < family->GetFontCount(); ++fontIndex) {
            ComPtr<IDWriteFont> font;
            if (FAILED(family->GetFont(fontIndex, &font))) continue;

            const auto fullNames = getInformationalStrings(font.Get(), DWRITE_INFORMATIONAL_STRING_FULL_NAME);
            const auto postScriptNames = getInformationalStrings(font.Get(), DWRITE_INFORMATIONAL_STRING_POSTSCRIPT_NAME);
            const bool matchesFullName = fullNames && containsName(fullNames.Get(), *wideName);
            const bool matchesPostScriptName = postScriptNames && containsName(postScriptNames.Get(), *wideName);
            if (!matchesFullName && !matchesPostScriptName) continue;
            if (const auto match = makeMatch(font.Get())) return match;
        }
    }
    return std::nullopt;
}
} // namespace radia::ui::detail
