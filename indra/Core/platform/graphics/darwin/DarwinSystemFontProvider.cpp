/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include <CoreFoundation/CoreFoundation.h>
#include <CoreText/CoreText.h>
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <limits>
#include <memory>
#include <string>
#include <string_view>
#include <type_traits>
#include "SystemFontProvider.h"

namespace Core::detail {
namespace {
template<typename T> using CFObject = std::unique_ptr<std::remove_pointer_t<T>, decltype(&CFRelease)>;

template<typename T> CFObject<T> own(T value) { return CFObject<T> {value, CFRelease}; }

CFObject<CFStringRef> createString(std::string_view value) {
    if (value.size() > static_cast<std::size_t>(std::numeric_limits<CFIndex>::max()))
        return own<CFStringRef>(nullptr);
    return own(CFStringCreateWithBytes(kCFAllocatorDefault, reinterpret_cast<const UInt8*>(value.data()),
        static_cast<CFIndex>(value.size()), kCFStringEncodingUTF8, false));
}

std::string toUTF8(CFStringRef value) {
    if (!value)
        return {};
    const CFIndex maxSize = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), kCFStringEncodingUTF8);
    if (maxSize < 0 || maxSize == std::numeric_limits<CFIndex>::max())
        return {};

    std::string result(static_cast<std::size_t>(maxSize + 1), '\0');
    if (!CFStringGetCString(value, result.data(), static_cast<CFIndex>(result.size()), kCFStringEncodingUTF8))
        return {};
    result.resize(std::char_traits<char>::length(result.c_str()));
    return result;
}

CFObject<CFStringRef> copyStringAttribute(CTFontDescriptorRef descriptor, CFStringRef key) {
    return own(static_cast<CFStringRef>(CTFontDescriptorCopyAttribute(descriptor, key)));
}

bool equalsCaseInsensitive(CFStringRef left, CFStringRef right) {
    return left && right && CFStringCompare(left, right, kCFCompareCaseInsensitive) == kCFCompareEqualTo;
}

CFObject<CFDictionaryRef> createRequestedTraits(const Style::FontSelectionRequest& request) {
    const float requestedWeight = std::isfinite(request.weight.value) ? request.weight.value : 400.f;
    const float weight = std::clamp((requestedWeight - 400.f) / 500.f, -1.f, 1.f);
    const float requestedWidth = std::isfinite(request.width.percentage) ? request.width.percentage : 100.f;
    const float width = requestedWidth > 0.f ? std::clamp(std::log2(requestedWidth / 100.f), -1.f, 1.f) : -1.f;
    const float slant = request.style == Style::FontStyle::Normal ? 0.f : 0.2f;
    const std::int32_t symbolicTraits = request.style == Style::FontStyle::Normal ? 0 : static_cast<std::int32_t>(kCTFontItalicTrait);

    CFObject<CFNumberRef> weightNumber = own(CFNumberCreate(kCFAllocatorDefault, kCFNumberFloatType, &weight));
    CFObject<CFNumberRef> widthNumber = own(CFNumberCreate(kCFAllocatorDefault, kCFNumberFloatType, &width));
    CFObject<CFNumberRef> slantNumber = own(CFNumberCreate(kCFAllocatorDefault, kCFNumberFloatType, &slant));
    CFObject<CFNumberRef> symbolicNumber = own(CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &symbolicTraits));
    if (!weightNumber || !widthNumber || !slantNumber || !symbolicNumber)
        return own<CFDictionaryRef>(nullptr);

    const void* keys[] = {kCTFontWeightTrait, kCTFontWidthTrait, kCTFontSlantTrait, kCTFontSymbolicTrait};
    const void* values[] = {weightNumber.get(), widthNumber.get(), slantNumber.get(), symbolicNumber.get()};
    return own(CFDictionaryCreate(kCFAllocatorDefault, keys, values, 4, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));
}

CFObject<CTFontDescriptorRef> copyDescriptorWithTraits(CTFontDescriptorRef base, CFDictionaryRef traits) {
    const void* keys[] = {kCTFontTraitsAttribute};
    const void* values[] = {traits};
    CFObject<CFDictionaryRef> attributes =
        own(CFDictionaryCreate(kCFAllocatorDefault, keys, values, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));
    if (!attributes)
        return own<CTFontDescriptorRef>(nullptr);
    return own(CTFontDescriptorCreateCopyWithAttributes(base, attributes.get()));
}

CFObject<CTFontDescriptorRef> matchDescriptor(CTFontDescriptorRef descriptor) {
    return own(CTFontDescriptorCreateMatchingFontDescriptor(descriptor, nullptr));
}

std::optional<int> faceIndex(CFURLRef url, CFStringRef postScriptName) {
    CFObject<CFArrayRef> descriptors = own(CTFontManagerCreateFontDescriptorsFromURL(url));
    if (!descriptors || !postScriptName)
        return std::nullopt;

    // Core Text returns one descriptor per collection face, in file order.
    const CFIndex count = CFArrayGetCount(descriptors.get());
    for (CFIndex index = 0; index < count; ++index) {
        auto descriptor = static_cast<CTFontDescriptorRef>(CFArrayGetValueAtIndex(descriptors.get(), index));
        CFObject<CFStringRef> candidate = copyStringAttribute(descriptor, kCTFontNameAttribute);
        if (!equalsCaseInsensitive(candidate.get(), postScriptName))
            continue;
        if (index > std::numeric_limits<int>::max())
            return std::nullopt;
        return static_cast<int>(index);
    }
    return std::nullopt;
}

std::optional<SystemFontMatch> makeMatch(CTFontDescriptorRef descriptor) {
    CFObject<CFURLRef> url = own(static_cast<CFURLRef>(CTFontDescriptorCopyAttribute(descriptor, kCTFontURLAttribute)));
    if (!url)
        return std::nullopt;
    CFObject<CFStringRef> path = own(CFURLCopyFileSystemPath(url.get(), kCFURLPOSIXPathStyle));
    if (!path)
        return std::nullopt;

    CFObject<CTFontRef> font = own(CTFontCreateWithFontDescriptor(descriptor, 0.f, nullptr));
    if (!font)
        return std::nullopt;
    CFObject<CFStringRef> familyName = own(CTFontCopyFamilyName(font.get()));
    CFObject<CFStringRef> fullName = own(CTFontCopyFullName(font.get()));
    CFObject<CFStringRef> postScriptName = own(CTFontCopyPostScriptName(font.get()));
    if (!familyName)
        familyName = copyStringAttribute(descriptor, kCTFontFamilyNameAttribute);
    if (!fullName)
        fullName = copyStringAttribute(descriptor, kCTFontDisplayNameAttribute);
    if (!postScriptName)
        postScriptName = copyStringAttribute(descriptor, kCTFontNameAttribute);

    const std::string fontPath = toUTF8(path.get());
    const std::optional<int> index = faceIndex(url.get(), postScriptName.get());
    if (fontPath.empty() || !index)
        return std::nullopt;

    return SystemFontMatch {fontPath, *index, toUTF8(familyName.get()), toUTF8(fullName.get()), toUTF8(postScriptName.get())};
}

std::optional<SystemFontMatch> matchFamilyName(CFStringRef familyName, const Style::FontSelectionRequest& request) {
    CFObject<CFDictionaryRef> traits = createRequestedTraits(request);
    if (!traits)
        return std::nullopt;
    const void* keys[] = {kCTFontFamilyNameAttribute};
    const void* values[] = {familyName};
    CFObject<CFDictionaryRef> attributes =
        own(CFDictionaryCreate(kCFAllocatorDefault, keys, values, 1, &kCFTypeDictionaryKeyCallBacks, &kCFTypeDictionaryValueCallBacks));
    if (!attributes)
        return std::nullopt;
    CFObject<CTFontDescriptorRef> base = own(CTFontDescriptorCreateWithAttributes(attributes.get()));
    if (!base)
        return std::nullopt;
    CFObject<CTFontDescriptorRef> requested = copyDescriptorWithTraits(base.get(), traits.get());
    if (!requested)
        return std::nullopt;
    CFObject<CTFontDescriptorRef> matched = matchDescriptor(requested.get());
    if (!matched)
        return std::nullopt;

    CFObject<CFStringRef> matchedFamily = copyStringAttribute(matched.get(), kCTFontFamilyNameAttribute);
    if (!equalsCaseInsensitive(matchedFamily.get(), familyName))
        return std::nullopt;
    return makeMatch(matched.get());
}

std::optional<SystemFontMatch> matchSystemUIFont(const Style::FontSelectionRequest& request) {
    CFObject<CTFontRef> systemFont = own(CTFontCreateUIFontForLanguage(kCTFontUIFontSystem, 12.f, nullptr));
    if (!systemFont)
        return std::nullopt;
    CFObject<CFStringRef> familyName = own(CTFontCopyFamilyName(systemFont.get()));
    if (!familyName)
        return std::nullopt;
    return matchFamilyName(familyName.get(), request);
}

const char* genericFamilyName(Style::GenericFontFamily family) {
    switch (family) {
    case Style::GenericFontFamily::Serif:
        return "Times New Roman";
    case Style::GenericFontFamily::SansSerif:
        return "Helvetica Neue";
    case Style::GenericFontFamily::SystemUI:
    case Style::GenericFontFamily::UISansSerif:
        return nullptr;
    case Style::GenericFontFamily::Cursive:
        return "Apple Chancery";
    case Style::GenericFontFamily::Fantasy:
        return "Papyrus";
    case Style::GenericFontFamily::Math:
        return "STIX Two Math";
    case Style::GenericFontFamily::Monospace:
        return "Menlo";
    case Style::GenericFontFamily::UISerif:
        return "New York";
    case Style::GenericFontFamily::UIMonospace:
        return "SF Mono";
    case Style::GenericFontFamily::UIRounded:
        return "SF Pro Rounded";
    case Style::GenericFontFamily::Fangsong:
        return "STFangsong";
    case Style::GenericFontFamily::Kai:
        return "STKaiti";
    case Style::GenericFontFamily::KhmerMul:
        return "Khmer MN";
    case Style::GenericFontFamily::Nastaliq:
        return "Noto Nastaliq Urdu";
    }
    return nullptr;
}

bool descriptorMatchesLocalName(CTFontDescriptorRef descriptor, CFStringRef expected) {
    CFObject<CFStringRef> postScriptName = copyStringAttribute(descriptor, kCTFontNameAttribute);
    if (equalsCaseInsensitive(postScriptName.get(), expected))
        return true;

    CFObject<CTFontRef> font = own(CTFontCreateWithFontDescriptor(descriptor, 0.f, nullptr));
    if (font) {
        CFObject<CFStringRef> fullName = own(CTFontCopyFullName(font.get()));
        if (equalsCaseInsensitive(fullName.get(), expected))
            return true;
    }
    CFObject<CFStringRef> displayName = copyStringAttribute(descriptor, kCTFontDisplayNameAttribute);
    return equalsCaseInsensitive(displayName.get(), expected);
}
} // namespace

std::optional<SystemFontMatch> matchSystemFont(const Style::FontFamily& family, const Style::FontSelectionRequest& request) {
    if (const auto* name = std::get_if<std::string>(&family)) {
        if (name->empty())
            return std::nullopt;
        CFObject<CFStringRef> familyName = createString(*name);
        if (!familyName)
            return std::nullopt;
        return matchFamilyName(familyName.get(), request);
    }

    const Style::GenericFontFamily genericFamily = std::get<Style::GenericFontFamily>(family);
    if (genericFamily == Style::GenericFontFamily::SystemUI || genericFamily == Style::GenericFontFamily::UISansSerif)
        return matchSystemUIFont(request);
    const char* mappedName = genericFamilyName(genericFamily);
    if (!mappedName)
        return std::nullopt;
    CFObject<CFStringRef> familyName = createString(mappedName);
    if (!familyName)
        return std::nullopt;
    return matchFamilyName(familyName.get(), request);
}

std::optional<SystemFontMatch> matchLocalFont(std::string_view name) {
    if (name.empty())
        return std::nullopt;
    CFObject<CFStringRef> expected = createString(name);
    if (!expected)
        return std::nullopt;
    CFObject<CTFontCollectionRef> collection = own(CTFontCollectionCreateFromAvailableFonts(nullptr));
    if (!collection)
        return std::nullopt;
    CFObject<CFArrayRef> descriptors = own(CTFontCollectionCreateMatchingFontDescriptors(collection.get()));
    if (!descriptors)
        return std::nullopt;

    const CFIndex count = CFArrayGetCount(descriptors.get());
    for (CFIndex index = 0; index < count; ++index) {
        auto descriptor = static_cast<CTFontDescriptorRef>(CFArrayGetValueAtIndex(descriptors.get(), index));
        if (!descriptorMatchesLocalName(descriptor, expected.get()))
            continue;
        return makeMatch(descriptor);
    }
    return std::nullopt;
}
} // namespace Core::detail
