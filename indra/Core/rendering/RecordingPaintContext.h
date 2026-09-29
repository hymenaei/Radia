/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>
#include "PaintContext.h"

namespace Core {
enum class PaintCommandKind {
    BeginFrame,
    EndFrame,
    PushClip,
    PopClip,
    PushTranslation,
    PopTranslation,
    BeginEffects,
    EndEffects,
    Scrollbar,
    NativeInput,
    NativeInputMark,
    NativeButton,
    Box,
    Text
};

struct PaintCommand {
    PaintCommandKind kind;
    Layout::Rect rect;
    Style::ComputedStyle style;
    std::string text;
    float scale = 1.f;
    Layout::ClipAxes clipAxes = Layout::ClipAxes::Both;
    std::optional<TopBorderGap> topBorderGap;
    std::optional<BackgroundPaintContext> backgroundPaintContext;
    Layout::Vec2 translation;
    std::optional<NativeScrollbarPaintRequest> scrollbar;
    std::optional<NativeInputPaintRequest> nativeInput;
    std::optional<NativeInputMarkPaintRequest> nativeInputMark;
    std::optional<NativeButtonPaintRequest> nativeButton;
    PaintTarget target;
};

class RecordingPaintContext final : public PaintContext {
public:
    explicit RecordingPaintContext(const TextMeasurer& textMetrics = fixedTextMeasurer())
        : mTextMeasurer(textMetrics) {}

    const TextMeasurer& textMetrics() const override { return mTextMeasurer; }
    Layout::Vec2 measureText(const std::string& text, const Style::ComputedStyle& style) const override;
    float usedLetterSpacing(const Style::ComputedStyle& style) const override;
    std::uint64_t generation() const noexcept override { return mTextMeasurer.generation(); }
    void beginFrame(const PaintTarget& target) override;
    void endFrame() override;
    void pushClip(const Layout::Rect& rect, float scale, Layout::ClipAxes axes = Layout::ClipAxes::Both) override;
    void popClip() override;
    void pushTranslation(const Layout::Vec2& translation) override;
    void popTranslation() override;
    void beginEffects(const Layout::Rect& rect, const Style::ComputedStyle& style, float scale) override;
    void endEffects() override;
    void paintNativeScrollbar(const NativeScrollbarPaintRequest& request) override;
    void paintNativeInput(const NativeInputPaintRequest& request) override;
    void paintNativeInputMark(const NativeInputMarkPaintRequest& request) override;
    void paintNativeButton(const NativeButtonPaintRequest& request) override;
    void paintBox(const Layout::Rect& rect, const Style::ComputedStyle& style,
        std::optional<TopBorderGap> topBorderGap = std::nullopt) override;
    void paintText(const std::string& text, const Layout::Rect& rect, const Style::ComputedStyle& style) override;
    const std::vector<PaintCommand>& commands() const { return mCommands; }
    std::size_t count(PaintCommandKind kind) const;
    const PaintCommand* last(PaintCommandKind kind) const;
    int clipDepth() const { return mClipDepth; }
    int maxClipDepth() const { return mMaxClipDepth; }
    int translationDepth() const { return mTranslationDepth; }
    int maxTranslationDepth() const { return mMaxTranslationDepth; }
    void clear();

private:
    const TextMeasurer& mTextMeasurer;
    std::vector<PaintCommand> mCommands;
    int mClipDepth = 0;
    int mMaxClipDepth = 0;
    int mTranslationDepth = 0;
    int mMaxTranslationDepth = 0;
};
} // namespace Core
