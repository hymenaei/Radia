#include "linden_common.h"
#include "rduitexthost.h"
#include "rduipaintcontext.h"
#include "rduistyle.h"
#include "rduistylesheet.h"
#include "rduitextmetrics.h"
#include "rduiwidget.h"
#include "llstring.h"
#include <algorithm>
#include <cmath>
#include <fribidi.h>
#include <limits>

namespace rdui
{
    namespace
    {
        struct TextRun
        {
            struct Key
            {
                std::string value;
                Style style;
                Vec2 text_size;
                Vec2 box_size;
                Vec2 size;
            };

            std::string value;
            Style style;
            Vec2 size;
            Vec2 box_size;
            std::vector<Key> keys;

            bool keybinding() const { return !keys.empty(); }
        };

        using TextLine = std::vector<TextRun>;

        Style inlineStyle(const StyleSheet* theme, const Widget& owner,
                          const std::vector<std::string>& inline_ancestors,
                          const Style& inherited)
        {
            Style style = theme ? theme->resolveInline(owner, "kbd", inline_ancestors) : Style{};
            inheritStyle(style, inherited);
            return style;
        }

        void appendNode(const InlineContentNode& node, const Style& inherited, const TextMetrics& metrics,
                        const StyleSheet* theme, const Widget& owner, std::vector<TextLine>& lines)
        {
            Style style = inherited;
            switch (node.kind())
            {
                case InlineContentKind::B: style.font_bold = true; break;
                case InlineContentKind::I: style.font_italic = true; break;
                case InlineContentKind::S: style.font_strike = true; break;
                case InlineContentKind::Br:
                    lines.emplace_back();
                    return;
                default: break;
            }

            if (node.kind() == InlineContentKind::Text)
            {
                const std::string& value = node.value();
                if (!value.empty()) lines.back().push_back({value, style, metrics.measureText(value, style)});
                return;
            }
            if (node.kind() == InlineContentKind::Kbd)
            {
                const KeybindingPresentation& presentation = node.keybindingPresentation();
                if (presentation.keys.empty()) return;

                TextRun run;
                run.style = inlineStyle(theme, owner, {}, style);
                const Style key_style = inlineStyle(theme, owner, {"kbd"}, run.style);
                const float gap = run.style.gap.fixedPixels();
                float content_width = 0.f;
                float content_height = 0.f;
                for (const std::string& key : presentation.keys)
                {
                    if (key.empty()) continue;
                    TextRun::Key key_run;
                    key_run.value = key;
                    key_run.style = key_style;
                    key_run.text_size = metrics.measureText(key, key_style);
                    key_run.box_size = {key_run.text_size.x + key_style.padding.horizontal(), key_run.text_size.y + key_style.padding.vertical()};
                    key_run.size = {key_run.box_size.x + key_style.margin.horizontal(), key_run.box_size.y + key_style.margin.vertical()};
                    if (!run.keys.empty())
                    {
                        content_width += gap;
                        run.value += ' ';
                    }
                    content_width += key_run.size.x;
                    content_height = std::max(content_height, key_run.size.y);
                    run.value += key;
                    run.keys.push_back(std::move(key_run));
                }
                if (run.keys.empty()) return;
                run.box_size = {content_width + run.style.padding.horizontal(), content_height + run.style.padding.vertical()};
                run.size = {run.box_size.x + run.style.margin.horizontal(), run.box_size.y + run.style.margin.vertical()};
                lines.back().push_back(std::move(run));
                return;
            }
            for (const InlineContentNode& child : node.children()) appendNode(child, style, metrics, theme, owner, lines);
        }

        std::vector<TextLine> layoutLines(const InlineContent& content, const Style& style,
                                          const TextMetrics& metrics, const StyleSheet* theme,
                                          const Widget& owner)
        {
            std::vector<TextLine> lines(1);
            for (const InlineContentNode& node : content.nodes()) appendNode(node, style, metrics, theme, owner, lines);
            return lines;
        }

        Vec2 lineSize(const TextLine& line, float fallback_height)
        {
            Vec2 size{0.f, fallback_height};
            for (const TextRun& run : line)
            {
                size.x += run.size.x;
                size.y = std::max(size.y, run.size.y);
            }
            return size;
        }

        std::vector<TextRun> visualRuns(const TextLine& line, LayoutDirection direction, const TextMetrics& metrics)
        {
            if (line.size() < 2) return line;

            std::vector<FriBidiChar> logical;
            std::vector<std::pair<std::size_t, std::size_t>> ranges;
            ranges.reserve(line.size());
            for (const TextRun& run : line)
            {
                const std::size_t begin = logical.size();
                if (run.keybinding()) logical.push_back(0xfffc);
                else
                {
                    const LLWString wide = utf8str_to_wstring(run.value);
                    for (llwchar character : wide) logical.push_back(static_cast<FriBidiChar>(character));
                }
                ranges.emplace_back(begin, logical.size());
            }
            if (logical.empty() || logical.size() > static_cast<std::size_t>(std::numeric_limits<FriBidiStrIndex>::max())) return line;

            std::vector<FriBidiStrIndex> logical_to_visual(logical.size());
            std::vector<FriBidiLevel> levels(logical.size());
            FriBidiParType base_direction = direction == LayoutDirection::RightToLeft
                                          ? FRIBIDI_PAR_RTL : FRIBIDI_PAR_LTR;
            if (!fribidi_log2vis(logical.data(), static_cast<FriBidiStrIndex>(logical.size()),
                                 &base_direction, nullptr, logical_to_visual.data(), nullptr, levels.data()))
                return line;

            struct VisualRun
            {
                FriBidiStrIndex start = 0;
                TextRun run;
            };

            std::vector<VisualRun> segments;
            for (std::size_t run_index = 0; run_index < line.size(); ++run_index)
            {
                const auto [run_begin, run_end] = ranges[run_index];
                if (line[run_index].keybinding())
                {
                    segments.push_back({logical_to_visual[run_begin], line[run_index]});
                    continue;
                }
                std::size_t begin = run_begin;
                while (begin < run_end)
                {
                    std::size_t end = begin + 1;
                    while (end < run_end && levels[end] == levels[begin]) ++end;

                    FriBidiStrIndex visual_start = logical_to_visual[begin];
                    LLWString wide;
                    wide.reserve(end - begin);
                    for (std::size_t offset = begin; offset < end; ++offset)
                    {
                        visual_start = std::min(visual_start, logical_to_visual[offset]);
                        wide.push_back(static_cast<llwchar>(logical[offset]));
                    }
                    std::string value = wstring_to_utf8str(wide);
                    segments.push_back({visual_start, {value, line[run_index].style, metrics.measureText(value, line[run_index].style)}});
                    begin = end;
                }
            }
            std::stable_sort(segments.begin(), segments.end(), [](const VisualRun& left, const VisualRun& right)
            {
                return left.start < right.start;
            });

            std::vector<TextRun> result;
            result.reserve(segments.size());
            for (VisualRun& segment : segments) result.push_back(std::move(segment.run));
            return result;
        }

        float alignedOffset(float available, float occupied, TextAlign alignment)
        {
            if (alignment == TextAlign::Center) return (available - occupied) * .5f;
            if (alignment == TextAlign::Right || alignment == TextAlign::End) return available - occupied;
            return 0.f;
        }

        void appendPlainText(const InlineContentNode& node, std::string& text)
        {
            if (node.kind() == InlineContentKind::Text)
            {
                text += node.value();
                return;
            }
            if (node.kind() == InlineContentKind::Kbd)
            {
                bool first = true;
                for (const std::string& key : node.keybindingPresentation().keys)
                {
                    if (!first) text += ' ';
                    text += key;
                    first = false;
                }
                return;
            }
            if (node.kind() == InlineContentKind::Br)
            {
                text += '\n';
                return;
            }
            for (const InlineContentNode& child : node.children()) appendPlainText(child, text);
        }

        bool hasKeybinding(const InlineContentNode& node)
        {
            if (node.kind() == InlineContentKind::Kbd) return true;
            return std::any_of(node.children().begin(), node.children().end(),
                               [](const InlineContentNode& child) { return hasKeybinding(child); });
        }

        void appendKeybindingSignature(const InlineContentNode& node, std::string& signature)
        {
            if (node.kind() == InlineContentKind::Kbd)
            {
                signature += node.metadata();
                signature += '\x1d';
                for (const std::string& key : node.keybindingPresentation().keys)
                {
                    signature += key;
                    signature += '\x1e';
                }
                signature += '\x1f';
                return;
            }
            for (const InlineContentNode& child : node.children())
                appendKeybindingSignature(child, signature);
        }

        std::string keybindingSignature(const InlineContent& content)
        {
            std::string signature;
            for (const InlineContentNode& node : content.nodes()) appendKeybindingSignature(node, signature);
            return signature;
        }
    }

    void TextHost::setContent(TextSource content)
    {
        mSource = std::move(content);
        mContent = mSource.materialize();
        mHasKeybindings = std::any_of(mContent.nodes().begin(), mContent.nodes().end(), [](const InlineContentNode& node) { return hasKeybinding(node); });
        updatePlainText();
    }

    void TextHost::resolveLocalized(const std::function<InlineContent(const LocalizationRequest&)>& resolve)
    {
        mContent = mSource.materialize(resolve);
        mHasKeybindings = std::any_of(
            mContent.nodes().begin(), mContent.nodes().end(),
            [](const InlineContentNode& node) { return hasKeybinding(node); });
        updatePlainText();
    }

    bool TextHost::resolveKeybindings(const std::function<KeybindingPresentation(const std::string&)>& resolve)
    {
        if (!mHasKeybindings) return false;
        const std::string previous = keybindingSignature(mContent);
        mContent = mContent.resolveKeybindings(resolve);
        updatePlainText();
        return keybindingSignature(mContent) != previous;
    }

    void TextHost::updatePlainText()
    {
        mPlainText.clear();
        for (const InlineContentNode& node : mContent.nodes()) appendPlainText(node, mPlainText);
    }

    Vec2 TextHost::measure(const TextMetrics& metrics, const Style& style, const StyleSheet& theme, const Widget& owner) const
    {
        const float fallback_height = metrics.measureText({}, style).y;
        Vec2 result;
        for (const TextLine& line : layoutLines(mContent, style, metrics, &theme, owner))
        {
            const Vec2 size = lineSize(line, fallback_height);
            result.x = std::max(result.x, size.x);
            result.y += size.y;
        }
        return result;
    }

    void TextHost::paint(PaintContext& context, const Rect& rect, const Style& style, const StyleSheet* theme, const Widget& owner) const
    {
        const std::vector<TextLine> lines = layoutLines(mContent, style, context, theme, owner);
        const float fallback_height = context.measureText({}, style).y;
        float y = rect.top();
        for (const TextLine& line : lines)
        {
            const TextLine visual_line = visualRuns(line, style.direction, context);
            const Vec2 size = lineSize(visual_line, fallback_height);
            y -= size.y;
            float x = rect.x + alignedOffset(rect.w, size.x, style.text_align);
            for (const TextRun& run : visual_line)
            {
                if (run.keybinding())
                {
                    const float run_bottom = y + (size.y - run.size.y) * .5f;
                    const Rect box{x + run.style.margin.left.fixedPixels(),
                                   run_bottom + run.style.margin.bottom.fixedPixels(),
                                   run.box_size.x, run.box_size.y};
                    context.paintBox(box, run.style);
                    float key_x = box.x + run.style.padding.left;
                    const float content_height = std::max(0.f, box.h - run.style.padding.vertical());
                    for (std::size_t index = 0; index < run.keys.size(); ++index)
                    {
                        const TextRun::Key& key = run.keys[index];
                        key_x += key.style.margin.left.fixedPixels();
                        const float key_y = box.y + run.style.padding.bottom
                                          + (content_height - key.size.y) * .5f
                                          + key.style.margin.bottom.fixedPixels();
                        const Rect key_box{key_x, key_y, key.box_size.x, key.box_size.y};
                        context.paintBox(key_box, key.style);
                        Style text_style = key.style;
                        text_style.text_align = TextAlign::Left;
                        context.paintText(key.value,
                                          {key_box.x + key.style.padding.left,
                                           key_box.y + key.style.padding.bottom,
                                           key.text_size.x, key.text_size.y},
                                          text_style);
                        key_x += key.box_size.x + key.style.margin.right.fixedPixels();
                        if (index + 1 < run.keys.size()) key_x += run.style.gap.fixedPixels();
                    }
                    x += run.size.x;
                    continue;
                }
                Style run_style = run.style;
                run_style.text_align = TextAlign::Left;
                context.paintText(run.value, {x, y, run.size.x, size.y}, run_style);
                x += run.size.x;
            }
        }
    }
}
