/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>
#include "render/paintcontext.h"

namespace radia::ui {
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
    Text,
    Icon
};

struct PaintCommand {
    PaintCommandKind kind;
    Rect rect;
    ComputedStyle style;
    std::string textOrIconName;
    float scale = 1.f;
    ClipAxes clipAxes = ClipAxes::Both;
    std::optional<TopBorderGap> topBorderGap;
    Vec2 translation;
    std::optional<NativeScrollbarPaintRequest> scrollbar;
    std::optional<NativeInputPaintRequest> nativeInput;
    std::optional<NativeInputMarkPaintRequest> nativeInputMark;
    std::optional<NativeButtonPaintRequest> nativeButton;
    PaintTarget target;
};

class RecordingPaintContext final : public PaintContext {
public:
    explicit RecordingPaintContext(const TextMetrics& textMetrics = fixedTextMetrics()) : mTextMetrics(textMetrics) {}

    Vec2 measureText(const std::string& text, const ComputedStyle& style) const override;
    float usedLetterSpacing(const ComputedStyle& style) const override;
    std::uint64_t generation() const noexcept override { return mTextMetrics.generation(); }
    void beginFrame(const PaintTarget& target) override;
    void endFrame() override;
    void pushClip(const Rect& rect, float scale, ClipAxes axes = ClipAxes::Both) override;
    void popClip() override;
    void pushTranslation(const Vec2& translation) override;
    void popTranslation() override;
    void beginEffects(const Rect& rect, const ComputedStyle& style, float scale) override;
    void endEffects() override;
    void paintNativeScrollbar(const NativeScrollbarPaintRequest& request) override;
    void paintNativeInput(const NativeInputPaintRequest& request) override;
    void paintNativeInputMark(const NativeInputMarkPaintRequest& request) override;
    void paintNativeButton(const NativeButtonPaintRequest& request) override;
    void paintBox(const Rect& rect, const ComputedStyle& style, std::optional<TopBorderGap> topBorderGap = std::nullopt) override;
    void paintText(const std::string& text, const Rect& rect, const ComputedStyle& style) override;
    void paintIcon(const std::string& name, const Rect& rect, const ComputedStyle& style, float scale) override;

    const std::vector<PaintCommand>& commands() const { return mCommands; }
    std::size_t count(PaintCommandKind kind) const;
    const PaintCommand* last(PaintCommandKind kind) const;
    int clipDepth() const { return mClipDepth; }
    int maxClipDepth() const { return mMaxClipDepth; }
    int translationDepth() const { return mTranslationDepth; }
    int maxTranslationDepth() const { return mMaxTranslationDepth; }
    void clear();

private:
    const TextMetrics& mTextMetrics;
    std::vector<PaintCommand> mCommands;
    int mClipDepth = 0;
    int mMaxClipDepth = 0;
    int mTranslationDepth = 0;
    int mMaxTranslationDepth = 0;
};
} // namespace radia::ui
