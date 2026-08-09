/**
 * @file layout.cpp
 * @brief
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

#include "linden_common.h"
#include "text/layout.h"
#include <algorithm>
#include <cwctype>
#include <limits>
#include <memory>
#include <fribidi.h>
#include <unicode/ubrk.h>
#include <unicode/utf16.h>
#include "llstring.h"
#include "text/metrics.h"

namespace rdui::detail {
namespace {
struct TextAtom {
    TextRun run;
    std::size_t source = 0;
    bool whitespace = false;
};

using TextChunk = std::vector<TextAtom>;

struct SourceRange {
    std::size_t begin;
    std::size_t end;
    std::size_t source;
};

struct LogicalLine {
    LLWString value;
    std::vector<SourceRange> sources;
};

Vec2 lineSize(const TextLine& line, float fallback_height, const TextMetrics& metrics) {
    Vec2 size{0.f, fallback_height};
    for (std::size_t index = 0; index < line.size(); ++index) {
        const TextRun& run = line[index];
        size.x += run.size.x;
        if (index) size.x += interRunSpacing(line[index - 1], run, metrics);
        size.y = std::max(size.y, run.size.y);
    }
    return size;
}

TextRun makeRun(std::string value, const Style& style, const TextMetrics& metrics) {
    return {value, style, metrics.measureText(value, style)};
}

bool whitespace(const LLWString& value) {
    return !value.empty()
        && std::all_of(value.begin(), value.end(), [](llwchar character) { return std::iswspace(static_cast<wint_t>(character)) != 0; });
}

LogicalLine logicalLine(const TextLine& line) {
    LogicalLine result;
    result.sources.reserve(line.size());
    for (std::size_t source = 0; source < line.size(); ++source) {
        const std::size_t begin = result.value.size();
        if (line[source].keybinding()) result.value.push_back(0xfffc);
        else result.value += utf8str_to_wstring(line[source].value);
        result.sources.push_back({begin, result.value.size(), source});
    }
    return result;
}

std::vector<std::size_t> unicodeBoundaries(const LLWString& wide, UBreakIteratorType type) {
    std::vector<UChar> utf16;
    std::vector<int32_t> utf16_offsets;
    utf16.reserve(wide.size());
    utf16_offsets.reserve(wide.size() + 1);
    utf16_offsets.push_back(0);
    for (llwchar character : wide) {
        const UChar32 codepoint = static_cast<UChar32>(character);
        if (codepoint <= 0xffff) utf16.push_back(static_cast<UChar>(codepoint));
        else {
            utf16.push_back(U16_LEAD(codepoint));
            utf16.push_back(U16_TRAIL(codepoint));
        }
        utf16_offsets.push_back(static_cast<int32_t>(utf16.size()));
    }

    UErrorCode status = U_ZERO_ERROR;
    std::unique_ptr<UBreakIterator, decltype(&ubrk_close)> iterator(
        ubrk_open(type, nullptr, utf16.data(), static_cast<int32_t>(utf16.size()), &status), &ubrk_close);
    if (U_FAILURE(status) || !iterator) return {};

    std::vector<std::size_t> result;
    for (int32_t boundary = ubrk_first(iterator.get()); boundary != UBRK_DONE; boundary = ubrk_next(iterator.get())) {
        const auto found = std::lower_bound(utf16_offsets.begin(), utf16_offsets.end(), boundary);
        if (found != utf16_offsets.end() && *found == boundary) result.push_back(static_cast<std::size_t>(found - utf16_offsets.begin()));
    }
    return result;
}
} // namespace

std::vector<std::size_t> graphemeBoundaries(const LLWString& value) {
    return unicodeBoundaries(value, UBRK_CHARACTER);
}

namespace {
void appendStyledAtoms(std::vector<TextAtom>& atoms, const LLWString& value, std::size_t source, const Style& style, const TextMetrics& metrics) {
    std::size_t begin = 0;
    while (begin < value.size()) {
        const bool is_whitespace = std::iswspace(static_cast<wint_t>(value[begin])) != 0;
        std::size_t end = begin + 1;
        while (end < value.size() && (std::iswspace(static_cast<wint_t>(value[end])) != 0) == is_whitespace) ++end;
        atoms.push_back({
            makeRun(wstring_to_utf8str(value.substr(begin, end - begin)), style, metrics),
            source,
            is_whitespace,
        });
        begin = end;
    }
}

std::vector<TextChunk> lineBreakChunks(const TextLine& line, const TextMetrics& metrics) {
    const LogicalLine logical = logicalLine(line);

    std::vector<std::size_t> boundaries = unicodeBoundaries(logical.value, UBRK_LINE);
    if (boundaries.size() < 2) {
        boundaries = {0};
        for (std::size_t index = 1; index < logical.value.size(); ++index) {
            const bool previous_space = std::iswspace(static_cast<wint_t>(logical.value[index - 1])) != 0;
            const bool current_space = std::iswspace(static_cast<wint_t>(logical.value[index])) != 0;
            if (previous_space && !current_space) boundaries.push_back(index);
        }
        boundaries.push_back(logical.value.size());
    }

    std::vector<TextChunk> chunks;
    chunks.reserve(boundaries.size() - 1);
    for (std::size_t boundary = 1; boundary < boundaries.size(); ++boundary) {
        const std::size_t chunk_begin = boundaries[boundary - 1];
        const std::size_t chunk_end = boundaries[boundary];
        TextChunk chunk;
        for (const SourceRange& range : logical.sources) {
            const std::size_t begin = std::max(chunk_begin, range.begin);
            const std::size_t end = std::min(chunk_end, range.end);
            if (begin >= end) continue;
            const TextRun& run = line[range.source];
            if (run.keybinding()) chunk.push_back({run, range.source, false});
            else appendStyledAtoms(chunk, logical.value.substr(begin, end - begin), range.source, run.style, metrics);
        }
        if (!chunk.empty()) chunks.push_back(std::move(chunk));
    }
    return chunks;
}

std::vector<TextChunk> characterClusters(const TextLine& line, const TextMetrics& metrics) {
    const LogicalLine logical = logicalLine(line);
    std::vector<std::size_t> boundaries = graphemeBoundaries(logical.value);
    if (boundaries.size() < 2) {
        boundaries.resize(logical.value.size() + 1);
        for (std::size_t index = 0; index <= logical.value.size(); ++index) boundaries[index] = index;
    }

    std::vector<TextChunk> result;
    result.reserve(boundaries.size() - 1);
    for (std::size_t boundary = 1; boundary < boundaries.size(); ++boundary) {
        const std::size_t cluster_begin = boundaries[boundary - 1];
        const std::size_t cluster_end = boundaries[boundary];
        TextChunk cluster;
        for (const SourceRange& range : logical.sources) {
            const std::size_t begin = std::max(cluster_begin, range.begin);
            const std::size_t end = std::min(cluster_end, range.end);
            if (begin >= end) continue;
            const TextRun& run = line[range.source];
            if (run.keybinding()) cluster.push_back({run, range.source, false});
            else {
                const LLWString value = logical.value.substr(begin, end - begin);
                cluster.push_back({
                    makeRun(wstring_to_utf8str(value), run.style, metrics),
                    range.source,
                    whitespace(value),
                });
            }
        }
        if (!cluster.empty()) result.push_back(std::move(cluster));
    }
    return result;
}

void appendAtom(TextLine& line, const TextAtom& atom, std::size_t& previous_source) {
    if (!atom.run.keybinding() && !line.empty() && atom.source == previous_source) line.back().value += atom.run.value;
    else line.push_back(atom.run);
    previous_source = atom.source;
}

void measureRuns(TextLine& line, const TextMetrics& metrics) {
    for (TextRun& run : line)
        if (!run.keybinding()) run.size = metrics.measureText(run.value, run.style);
}

TextLine coalesce(const std::vector<TextAtom>& atoms, const TextMetrics& metrics) {
    TextLine result;
    std::size_t previous_source = std::numeric_limits<std::size_t>::max();
    for (const TextAtom& atom : atoms) appendAtom(result, atom, previous_source);
    measureRuns(result, metrics);
    return result;
}

TextLine select(const std::vector<TextChunk>& clusters, std::size_t prefix_count, const TextAtom* separator, std::size_t suffix_begin,
                const TextMetrics& metrics) {
    TextLine result;
    std::size_t previous_source = std::numeric_limits<std::size_t>::max();
    for (std::size_t index = 0; index < prefix_count; ++index)
        for (const TextAtom& atom : clusters[index]) appendAtom(result, atom, previous_source);
    if (separator) appendAtom(result, *separator, previous_source);
    for (std::size_t index = suffix_begin; index < clusters.size(); ++index)
        for (const TextAtom& atom : clusters[index]) appendAtom(result, atom, previous_source);
    measureRuns(result, metrics);
    return result;
}

class WrappedLine {
public:
    explicit WrappedLine(const TextMetrics& metrics) : mMetrics(metrics) {}

    struct Snapshot {
        std::size_t line_size = 0;
        std::optional<TextRun> previous_last;
        std::optional<std::size_t> previous_source;
        std::size_t pending_size = 0;
        float width = 0.f;
    };

    Snapshot snapshot() const {
        return {mLine.size(), mLine.empty() ? std::optional<TextRun>() : std::optional<TextRun>(mLine.back()), mLastSource, mPending.size(), mWidth};
    }

    void restore(const Snapshot& snapshot) {
        mLine.resize(snapshot.line_size);
        if (snapshot.previous_last && !mLine.empty()) mLine.back() = *snapshot.previous_last;
        mLastSource = snapshot.previous_source;
        mPending.resize(snapshot.pending_size);
        mWidth = snapshot.width;
    }

    void append(const TextChunk& chunk) {
        for (const TextAtom& atom : chunk) append(atom);
    }

    bool empty() const { return mLine.empty(); }
    float width() const { return mWidth; }

    TextLine finish() {
        TextLine result = std::move(mLine);
        mLine.clear();
        mPending.clear();
        mLastSource.reset();
        mWidth = 0.f;
        return result;
    }

private:
    void append(const TextAtom& atom) {
        if (atom.whitespace) {
            if (!mLine.empty()) mPending.push_back(atom);
            return;
        }

        for (const TextAtom& pending : mPending) appendRun(pending.run, pending.source);
        mPending.clear();
        appendRun(atom.run, atom.source);
    }

    void appendRun(const TextRun& run, std::size_t source) {
        if (!mLine.empty() && !run.keybinding() && !mLine.back().keybinding() && mLastSource == source) {
            const float previous_spacing = mLine.size() > 1 ? interRunSpacing(mLine[mLine.size() - 2], mLine.back(), mMetrics) : 0.f;
            mWidth -= mLine.back().size.x + previous_spacing;
            mLine.back().value += run.value;
            mLine.back().size = mMetrics.measureText(mLine.back().value, mLine.back().style);
            const float spacing = mLine.size() > 1 ? interRunSpacing(mLine[mLine.size() - 2], mLine.back(), mMetrics) : 0.f;
            mWidth += mLine.back().size.x + spacing;
            return;
        }

        if (!mLine.empty()) mWidth += interRunSpacing(mLine.back(), run, mMetrics);
        mLine.push_back(run);
        mLastSource = source;
        mWidth += run.size.x;
    }

    const TextMetrics& mMetrics;
    TextLine mLine;
    std::vector<TextAtom> mPending;
    std::optional<std::size_t> mLastSource;
    float mWidth = 0.f;
};

std::vector<TextLine> wrapLine(const TextLine& source, float available, float fallback_height, const TextMetrics& metrics) {
    if (source.empty() || lineSize(source, fallback_height, metrics).x <= available) return {source};

    const std::vector<TextChunk> chunks = lineBreakChunks(source, metrics);
    std::vector<TextLine> result;
    WrappedLine current(metrics);
    for (const TextChunk& chunk : chunks) {
        const WrappedLine::Snapshot before = current.snapshot();
        current.append(chunk);
        if (current.width() > available && before.line_size != 0) {
            current.restore(before);
            if (!current.empty()) result.push_back(current.finish());
            current.append(chunk);
        }
    }
    if (!current.empty() || result.empty()) result.push_back(current.finish());
    return result;
}

TextLine visualRuns(const TextLine& line, LayoutDirection direction, const TextMetrics& metrics) {
    if (line.size() < 2) return line;

    std::vector<FriBidiChar> logical;
    std::vector<std::pair<std::size_t, std::size_t>> ranges;
    ranges.reserve(line.size());
    for (const TextRun& run : line) {
        const std::size_t begin = logical.size();
        if (run.keybinding()) logical.push_back(0xfffc);
        else {
            const LLWString wide = utf8str_to_wstring(run.value);
            for (llwchar character : wide) logical.push_back(static_cast<FriBidiChar>(character));
        }
        ranges.emplace_back(begin, logical.size());
    }
    if (logical.empty() || logical.size() > static_cast<std::size_t>(std::numeric_limits<FriBidiStrIndex>::max())) return line;

    std::vector<FriBidiStrIndex> logical_to_visual(logical.size());
    std::vector<FriBidiLevel> levels(logical.size());
    FriBidiParType base_direction = direction == LayoutDirection::RightToLeft ? FRIBIDI_PAR_RTL : FRIBIDI_PAR_LTR;
    if (!fribidi_log2vis(logical.data(), static_cast<FriBidiStrIndex>(logical.size()), &base_direction, nullptr, logical_to_visual.data(), nullptr,
                         levels.data()))
        return line;

    struct VisualRun {
        FriBidiStrIndex start = 0;
        TextRun run;
    };

    std::vector<VisualRun> segments;
    for (std::size_t run_index = 0; run_index < line.size(); ++run_index) {
        const auto [run_begin, run_end] = ranges[run_index];
        if (line[run_index].keybinding()) {
            segments.push_back({logical_to_visual[run_begin], line[run_index]});
            continue;
        }
        std::size_t begin = run_begin;
        while (begin < run_end) {
            std::size_t end = begin + 1;
            while (end < run_end && levels[end] == levels[begin]) ++end;

            FriBidiStrIndex visual_start = logical_to_visual[begin];
            LLWString wide;
            wide.reserve(end - begin);
            for (std::size_t offset = begin; offset < end; ++offset) {
                visual_start = std::min(visual_start, logical_to_visual[offset]);
                wide.push_back(static_cast<llwchar>(logical[offset]));
            }
            const std::string value = wstring_to_utf8str(wide);
            segments.push_back({
                visual_start,
                makeRun(value, line[run_index].style, metrics),
            });
            begin = end;
        }
    }
    std::stable_sort(segments.begin(), segments.end(), [](const VisualRun& left, const VisualRun& right) { return left.start < right.start; });

    TextLine result;
    result.reserve(segments.size());
    for (VisualRun& segment : segments) result.push_back(std::move(segment.run));
    return result;
}

TextLine truncateLine(const TextLine& line, float available, float fallback_height, const Style& style, const TextMetrics& metrics) {
    if (lineSize(line, fallback_height, metrics).x <= available) return line;
    if (style.text_overflow == TextOverflow::Clip) return line;
    const std::vector<TextChunk> clusters = characterClusters(line, metrics);
    if (clusters.empty()) return {};

    const auto fits = [&](std::size_t prefix_count, const TextAtom* separator, std::size_t suffix_begin) {
        return lineSize(select(clusters, prefix_count, separator, suffix_begin, metrics), fallback_height, metrics).x <= available;
    };

    constexpr std::size_t ELLIPSIS_SOURCE = std::numeric_limits<std::size_t>::max();
    const TextAtom ellipsis{
        makeRun("\xE2\x80\xA6", style, metrics),
        ELLIPSIS_SOURCE,
        false,
    };
    if (!fits(0, &ellipsis, clusters.size())) return {};

    std::size_t prefix_count = 0;
    std::size_t suffix_begin = clusters.size();
    if (style.text_overflow == TextOverflow::Ellipsis) {
        std::size_t first_failing = clusters.size() + 1;
        while (prefix_count + 1 < first_failing) {
            const std::size_t candidate = prefix_count + (first_failing - prefix_count) / 2;
            if (fits(candidate, &ellipsis, clusters.size())) prefix_count = candidate;
            else first_failing = candidate;
        }
    } else {
        struct CenterSelection {
            std::size_t prefix;
            std::size_t suffix;
        };
        std::vector<CenterSelection> growth;
        growth.reserve(clusters.size() + 1);
        growth.push_back({prefix_count, suffix_begin});
        std::vector<float> cluster_widths;
        cluster_widths.reserve(clusters.size());
        for (const TextChunk& cluster : clusters) cluster_widths.push_back(lineSize(coalesce(cluster, metrics), fallback_height, metrics).x);
        float prefix_advance = 0.f;
        float suffix_advance = 0.f;
        while (prefix_count < suffix_begin) {
            const float next_prefix = prefix_advance + cluster_widths[prefix_count];
            const float next_suffix = suffix_advance + cluster_widths[suffix_begin - 1];
            if (next_prefix <= next_suffix) {
                prefix_advance = next_prefix;
                ++prefix_count;
            } else {
                suffix_advance = next_suffix;
                --suffix_begin;
            }
            growth.push_back({prefix_count, suffix_begin});
        }

        std::size_t selected = 0;
        std::size_t first_failing = growth.size();
        while (selected + 1 < first_failing) {
            const std::size_t candidate = selected + (first_failing - selected) / 2;
            const CenterSelection state = growth[candidate];
            if (fits(state.prefix, &ellipsis, state.suffix)) selected = candidate;
            else first_failing = candidate;
        }
        prefix_count = growth[selected].prefix;
        suffix_begin = growth[selected].suffix;
    }
    return select(clusters, prefix_count, &ellipsis, suffix_begin, metrics);
}
} // namespace

float interRunSpacing(const TextRun& left, const TextRun& right, const TextMetrics& metrics) {
    if (left.keybinding() || right.keybinding() || left.value.empty() || right.value.empty()) return 0.f;
    const LLWString left_wide = utf8str_to_wstring(left.value);
    LLWString joined = left_wide;
    joined += utf8str_to_wstring(right.value);
    const std::vector<std::size_t> boundaries = graphemeBoundaries(joined);
    if (std::find(boundaries.begin(), boundaries.end(), left_wide.size()) == boundaries.end()) return 0.f;
    return metrics.usedLetterSpacing(left.style);
}

TextLayout layoutText(const std::vector<TextLine>& hard_lines, const Style& style, const TextMetrics& metrics, std::optional<float> available_width,
                      bool visual_order, bool apply_overflow) {
    TextLayout result;
    const float fallback_height = metrics.measureText({}, style).y;
    for (const TextLine& hard_line : hard_lines) {
        std::vector<TextLine> visual_lines;
        if (available_width && style.text_wrap == TextWrap::Wrap) visual_lines = wrapLine(hard_line, *available_width, fallback_height, metrics);
        else visual_lines.push_back(hard_line);

        for (TextLine& line : visual_lines) {
            if (available_width && apply_overflow && style.text_wrap == TextWrap::NoWrap && style.overflow_x == Overflow::Hidden)
                line = truncateLine(line, *available_width, fallback_height, style, metrics);
            if (visual_order) line = visualRuns(line, style.direction, metrics);

            const Vec2 size = lineSize(line, fallback_height, metrics);
            result.size.x = std::max(result.size.x, size.x);
            result.size.y += size.y;
            result.lines.push_back({std::move(line), size});
        }
    }
    return result;
}
} // namespace rdui::detail
