/**
 * @file host.h
 * @brief Measures and paints TextHost content with localization, inline styles, and keybindings.
 *
 * $LicenseInfo:firstyear=2026&license=viewerlgpl$
 * Radia Viewer Source Code
 * Copyright (C) 2026, Hymenaei
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
 * $/LicenseInfo$
 */

#ifndef RD_TEXT_HOST_H
#define RD_TEXT_HOST_H

#include <cstddef>
#include <cstdint>
#include <optional>
#include "text/layout.h"
#include "text/source.h"
#include "types.h"

namespace rdui {
class StyleSheet;
class Widget;

class PaintContext;
struct Style;
class TextMetrics;

class TextHost {
public:
    TextHost() = default;
    explicit TextHost(TextSource content) { setContent(std::move(content)); }
    explicit TextHost(InlineContent content) { setContent(TextSource::literal(std::move(content))); }

    void setContent(TextSource content);
    void setContent(InlineContent content) { setContent(TextSource::literal(std::move(content))); }
    const InlineContent& content() const { return mContent; }
    const std::string& plainText() const { return mPlainText; }
    void resolveLocalized(const std::function<InlineContent(const LocalizationRequest&)>& resolve);
    bool resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve);

    Vec2 measure(const TextMetrics& metrics, const Style& style, const StyleSheet& theme, const Widget& owner,
                 std::optional<float> resolved_width = std::nullopt) const;
    void paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* theme, const Widget& owner) const;

private:
    void updatePlainText();
    const std::vector<detail::TextLine>& cachedLines(const TextMetrics& metrics, const Style& style, const StyleSheet* theme,
                                                     const Widget& owner) const;
    const detail::TextLayout& cachedLayout(const TextMetrics& metrics, const Style& style, const StyleSheet* theme, const Widget& owner,
                                           std::optional<float> available_width, bool visual_order, bool apply_overflow) const;

    TextSource mSource;
    InlineContent mContent;
    std::string mPlainText;
    bool mHasKeybindings = false;
    std::uint64_t mContentGeneration = 0;
    mutable std::uint64_t mCachedContentGeneration = 0;
    mutable const TextMetrics* mCachedMetrics = nullptr;
    mutable std::uint64_t mCachedMetricsGeneration = 0;
    mutable const StyleSheet* mCachedTheme = nullptr;
    mutable std::uint64_t mCachedThemeGeneration = 0;
    mutable const Widget* mCachedOwner = nullptr;
    mutable const Widget* mCachedOwnerParent = nullptr;
    mutable std::uint64_t mCachedOwnerStyleRevision = 0;
    mutable std::size_t mCachedStyleFingerprint = 0;
    mutable std::vector<detail::TextLine> mCachedLines;
    mutable bool mCachedLayoutValid = false;
    mutable bool mCachedLayoutWidthSet = false;
    mutable float mCachedLayoutWidth = 0.f;
    mutable bool mCachedLayoutVisualOrder = false;
    mutable bool mCachedLayoutOverflow = false;
    mutable std::size_t mCachedLayoutStyleFingerprint = 0;
    mutable detail::TextLayout mCachedLayout;
};
} // namespace rdui
#endif // RD_TEXT_HOST_H
