/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "TextLayout.h"
#include <algorithm>
#include <cstdint>
#include <cwctype>
#include <functional>
#include <limits>
#include <memory>
#include <utility>
#include <variant>
#include <fribidi.h>
#include <unicode/ubrk.h>
#include <unicode/utf16.h>
#include "ComputedStyle.h"
#include "Element.h"
#include "LayoutGeometry.h"
#include "PaintContext.h"
#include "StyleSheet.h"
#include "TextMeasurer.h"
#include "llstring.h"

namespace Core::Layout::detail {
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

Vec2 lineSize(const TextLine& line, float fallbackHeight, const TextMeasurer& metrics) {
    Vec2 size {0.f, fallbackHeight};
    for (std::size_t index = 0; index < line.size(); ++index) {
        const TextRun& run = line[index];
        size.x += run.size.x;
        if (index)
            size.x += interRunSpacing(line[index - 1], run, metrics);
        size.y = std::max(size.y, run.size.y);
    }
    return size;
}

TextRun makeRun(std::string value, const Style::ComputedStyle& style, const TextMeasurer& metrics) {
    return {value, style, metrics.measureText(value, style)};
}

bool whitespace(const LLWString& value) {
    return !value.empty() && std::all_of(value.begin(), value.end(), [](llwchar character) {
        return std::iswspace(static_cast<wint_t>(character)) != 0;
    });
}

LogicalLine logicalLine(const TextLine& line) {
    LogicalLine result;
    result.sources.reserve(line.size());
    for (std::size_t source = 0; source < line.size(); ++source) {
        const std::size_t begin = result.value.size();
        result.value += utf8str_to_wstring(line[source].value);
        result.sources.push_back({begin, result.value.size(), source});
    }
    return result;
}

std::vector<std::size_t> unicodeBoundaries(const LLWString& wide, UBreakIteratorType type) {
    std::vector<UChar> utf16;
    std::vector<int32_t> codepointToUtf16Offsets;
    utf16.reserve(wide.size());
    codepointToUtf16Offsets.reserve(wide.size() + 1);
    codepointToUtf16Offsets.push_back(0);
    for (llwchar character : wide) {
        const UChar32 codepoint = static_cast<UChar32>(character);
        if (codepoint <= 0xffff)
            utf16.push_back(static_cast<UChar>(codepoint));
        else {
            utf16.push_back(U16_LEAD(codepoint));
            utf16.push_back(U16_TRAIL(codepoint));
        }
        codepointToUtf16Offsets.push_back(static_cast<int32_t>(utf16.size()));
    }

    UErrorCode status = U_ZERO_ERROR;
    std::unique_ptr<UBreakIterator, decltype(&ubrk_close)> iterator(
        ubrk_open(type, nullptr, utf16.data(), static_cast<int32_t>(utf16.size()), &status), &ubrk_close);
    if (U_FAILURE(status) || !iterator)
        return {};

    std::vector<std::size_t> result;
    for (int32_t boundary = ubrk_first(iterator.get()); boundary != UBRK_DONE; boundary = ubrk_next(iterator.get())) {
        const auto found = std::lower_bound(codepointToUtf16Offsets.begin(), codepointToUtf16Offsets.end(), boundary);
        if (found != codepointToUtf16Offsets.end() && *found == boundary)
            result.push_back(static_cast<std::size_t>(found - codepointToUtf16Offsets.begin()));
    }
    return result;
}
} // namespace

std::vector<std::size_t> graphemeBoundaries(const LLWString& value) { return unicodeBoundaries(value, UBRK_CHARACTER); }

namespace {
void appendStyledAtoms(std::vector<TextAtom>& atoms, const LLWString& value, std::size_t source, const Style::ComputedStyle& style,
    const TextMeasurer& metrics) {
    std::size_t begin = 0;
    while (begin < value.size()) {
        const bool isWhitespace = std::iswspace(static_cast<wint_t>(value[begin])) != 0;
        std::size_t end = begin + 1;
        while (end < value.size() && (std::iswspace(static_cast<wint_t>(value[end])) != 0) == isWhitespace)
            ++end;
        atoms.push_back({
            makeRun(wstring_to_utf8str(value.substr(begin, end - begin)), style, metrics),
            source,
            isWhitespace,
        });
        begin = end;
    }
}

std::vector<TextChunk> lineBreakChunks(const TextLine& line, const TextMeasurer& metrics) {
    const LogicalLine logical = logicalLine(line);

    std::vector<std::size_t> boundaries = unicodeBoundaries(logical.value, UBRK_LINE);
    if (boundaries.size() < 2) {
        boundaries = {0};
        for (std::size_t index = 1; index < logical.value.size(); ++index) {
            const bool previousWhitespace = std::iswspace(static_cast<wint_t>(logical.value[index - 1])) != 0;
            const bool currentWhitespace = std::iswspace(static_cast<wint_t>(logical.value[index])) != 0;
            if (previousWhitespace && !currentWhitespace)
                boundaries.push_back(index);
        }
        boundaries.push_back(logical.value.size());
    }

    std::vector<TextChunk> chunks;
    chunks.reserve(boundaries.size() - 1);
    for (std::size_t boundary = 1; boundary < boundaries.size(); ++boundary) {
        const std::size_t chunkBegin = boundaries[boundary - 1];
        const std::size_t chunkEnd = boundaries[boundary];
        TextChunk chunk;
        for (const SourceRange& range : logical.sources) {
            const std::size_t begin = std::max(chunkBegin, range.begin);
            const std::size_t end = std::min(chunkEnd, range.end);
            if (begin >= end)
                continue;
            const TextRun& run = line[range.source];
            appendStyledAtoms(chunk, logical.value.substr(begin, end - begin), range.source, run.style, metrics);
        }
        if (!chunk.empty())
            chunks.push_back(std::move(chunk));
    }
    return chunks;
}

std::vector<TextChunk> characterClusters(const TextLine& line, const TextMeasurer& metrics) {
    const LogicalLine logical = logicalLine(line);
    std::vector<std::size_t> boundaries = graphemeBoundaries(logical.value);
    if (boundaries.size() < 2) {
        boundaries.resize(logical.value.size() + 1);
        for (std::size_t index = 0; index <= logical.value.size(); ++index)
            boundaries[index] = index;
    }

    std::vector<TextChunk> result;
    result.reserve(boundaries.size() - 1);
    for (std::size_t boundary = 1; boundary < boundaries.size(); ++boundary) {
        const std::size_t clusterBegin = boundaries[boundary - 1];
        const std::size_t clusterEnd = boundaries[boundary];
        TextChunk cluster;
        for (const SourceRange& range : logical.sources) {
            const std::size_t begin = std::max(clusterBegin, range.begin);
            const std::size_t end = std::min(clusterEnd, range.end);
            if (begin >= end)
                continue;
            const TextRun& run = line[range.source];
            const LLWString value = logical.value.substr(begin, end - begin);
            cluster.push_back({
                makeRun(wstring_to_utf8str(value), run.style, metrics),
                range.source,
                whitespace(value),
            });
        }
        if (!cluster.empty())
            result.push_back(std::move(cluster));
    }
    return result;
}

void appendAtom(TextLine& line, const TextAtom& atom, std::size_t& previousSourceIndex) {
    if (!line.empty() && atom.source == previousSourceIndex)
        line.back().value += atom.run.value;
    else
        line.push_back(atom.run);
    previousSourceIndex = atom.source;
}

void measureRuns(TextLine& line, const TextMeasurer& metrics) {
    for (TextRun& run : line)
        run.size = metrics.measureText(run.value, run.style);
}

TextLine coalesce(const std::vector<TextAtom>& atoms, const TextMeasurer& metrics) {
    TextLine result;
    std::size_t previousSourceIndex = std::numeric_limits<std::size_t>::max();
    for (const TextAtom& atom : atoms)
        appendAtom(result, atom, previousSourceIndex);
    measureRuns(result, metrics);
    return result;
}

TextLine select(const std::vector<TextChunk>& clusters, std::size_t prefixCount, const TextAtom* separator, std::size_t suffixBegin,
    const TextMeasurer& metrics) {
    TextLine result;
    std::size_t previousSourceIndex = std::numeric_limits<std::size_t>::max();
    for (std::size_t index = 0; index < prefixCount; ++index)
        for (const TextAtom& atom : clusters[index])
            appendAtom(result, atom, previousSourceIndex);
    if (separator)
        appendAtom(result, *separator, previousSourceIndex);
    for (std::size_t index = suffixBegin; index < clusters.size(); ++index)
        for (const TextAtom& atom : clusters[index])
            appendAtom(result, atom, previousSourceIndex);
    measureRuns(result, metrics);
    return result;
}

class WrappedLine {
public:
    explicit WrappedLine(const TextMeasurer& metrics)
        : mMetrics(metrics) {}

    struct Snapshot {
        std::size_t lineRunCount = 0;
        std::optional<TextRun> previousLastRun;
        std::optional<std::size_t> previousSourceIndex;
        std::size_t pendingAtomCount = 0;
        float width = 0.f;
    };

    Snapshot snapshot() const {
        return {mLine.size(), mLine.empty() ? std::optional<TextRun>() : std::optional<TextRun>(mLine.back()), mLastSource, mPending.size(),
            mWidth};
    }

    void restore(const Snapshot& snapshot) {
        mLine.resize(snapshot.lineRunCount);
        if (snapshot.previousLastRun && !mLine.empty())
            mLine.back() = *snapshot.previousLastRun;
        mLastSource = snapshot.previousSourceIndex;
        mPending.resize(snapshot.pendingAtomCount);
        mWidth = snapshot.width;
    }

    void append(const TextChunk& chunk) {
        for (const TextAtom& atom : chunk)
            append(atom);
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
            if (!mLine.empty())
                mPending.push_back(atom);
            return;
        }

        for (const TextAtom& pending : mPending)
            appendRun(pending.run, pending.source);
        mPending.clear();
        appendRun(atom.run, atom.source);
    }

    void appendRun(const TextRun& run, std::size_t source) {
        if (!mLine.empty() && mLastSource == source) {
            const float previousSpacing = mLine.size() > 1 ? interRunSpacing(mLine[mLine.size() - 2], mLine.back(), mMetrics) : 0.f;
            mWidth -= mLine.back().size.x + previousSpacing;
            mLine.back().value += run.value;
            mLine.back().size = mMetrics.measureText(mLine.back().value, mLine.back().style);
            const float spacing = mLine.size() > 1 ? interRunSpacing(mLine[mLine.size() - 2], mLine.back(), mMetrics) : 0.f;
            mWidth += mLine.back().size.x + spacing;
            return;
        }

        if (!mLine.empty())
            mWidth += interRunSpacing(mLine.back(), run, mMetrics);
        mLine.push_back(run);
        mLastSource = source;
        mWidth += run.size.x;
    }

    const TextMeasurer& mMetrics;
    TextLine mLine;
    std::vector<TextAtom> mPending;
    std::optional<std::size_t> mLastSource;
    float mWidth = 0.f;
};

std::vector<TextLine> wrapLine(const TextLine& source, float available, float fallbackHeight, const TextMeasurer& metrics) {
    if (source.empty() || lineSize(source, fallbackHeight, metrics).x <= available)
        return {source};

    const std::vector<TextChunk> chunks = lineBreakChunks(source, metrics);
    std::vector<TextLine> result;
    WrappedLine current(metrics);
    for (const TextChunk& chunk : chunks) {
        const WrappedLine::Snapshot before = current.snapshot();
        current.append(chunk);
        if (current.width() > available && before.lineRunCount != 0) {
            current.restore(before);
            if (!current.empty())
                result.push_back(current.finish());
            current.append(chunk);
        }
    }
    if (!current.empty() || result.empty())
        result.push_back(current.finish());
    return result;
}

std::vector<TextLine> optimizedWrapLine(const TextLine& source, float available, float fallbackHeight, const TextMeasurer& metrics,
    Style::TextWrapStyle optimization) {
    const std::vector<TextLine> greedy = wrapLine(source, available, fallbackHeight, metrics);
    if (greedy.size() < 2 || greedy.size() > 10)
        return greedy;

    const std::vector<TextChunk> chunks = lineBreakChunks(source, metrics);
    if (chunks.size() < greedy.size() || chunks.size() > 128)
        return greedy;

    const std::size_t chunkCount = chunks.size();
    std::vector<float> widths((chunkCount + 1) * (chunkCount + 1), std::numeric_limits<float>::quiet_NaN());
    const auto width = [&](std::size_t begin, std::size_t end) {
        float& cached = widths[begin * (chunkCount + 1) + end];
        if (std::isnan(cached)) {
            WrappedLine line(metrics);
            for (std::size_t index = begin; index < end; ++index)
                line.append(chunks[index]);
            cached = lineSize(line.finish(), fallbackHeight, metrics).x;
        }
        return cached;
    };
    const std::size_t lineCount = greedy.size();
    const float targetWidth = width(0, chunkCount) / static_cast<float>(lineCount);
    const float targetRemaining = available - targetWidth;
    struct State {
        float cost = std::numeric_limits<float>::infinity();
        std::size_t previous = 0;
    };
    std::vector<std::vector<State>> states(lineCount + 1, std::vector<State>(chunkCount + 1));
    states[0][0].cost = 0.f;
    for (std::size_t line = 1; line <= lineCount; ++line) {
        for (std::size_t end = line; end + (lineCount - line) <= chunkCount; ++end) {
            for (std::size_t begin = line - 1; begin < end; ++begin) {
                if (!std::isfinite(states[line - 1][begin].cost))
                    continue;
                const float lineWidth = width(begin, end);
                const float overflow = std::max(0.f, lineWidth - available);
                const float remaining = available - lineWidth;
                float penalty = 0.f;
                if (optimization == Style::TextWrapStyle::Balance) {
                    const float deviation = remaining - targetRemaining;
                    penalty = deviation * deviation;
                } else {
                    const float looseSpace = std::max(0.f, remaining);
                    penalty = looseSpace * looseSpace * (looseSpace + 1.f);
                    if (line == lineCount) {
                        const float shortLastLine = std::max(0.f, targetWidth - lineWidth);
                        penalty += shortLastLine * shortLastLine * 4.f;
                    }
                }
                penalty += overflow * overflow * 1000.f;
                const float cost = states[line - 1][begin].cost + penalty;
                if (cost < states[line][end].cost)
                    states[line][end] = {cost, begin};
            }
        }
    }
    if (!std::isfinite(states[lineCount].back().cost))
        return greedy;

    std::vector<std::pair<std::size_t, std::size_t>> ranges(lineCount);
    std::size_t end = chunkCount;
    for (std::size_t line = lineCount; line > 0; --line) {
        const std::size_t begin = states[line][end].previous;
        ranges[line - 1] = {begin, end};
        end = begin;
    }
    std::vector<TextLine> result;
    result.reserve(lineCount);
    for (const auto [begin, rangeEnd] : ranges) {
        WrappedLine line(metrics);
        for (std::size_t index = begin; index < rangeEnd; ++index)
            line.append(chunks[index]);
        result.push_back(line.finish());
    }
    return result;
}

std::vector<TextLine> balancedWrapLine(const TextLine& source, float available, float fallbackHeight, const TextMeasurer& metrics) {
    return optimizedWrapLine(source, available, fallbackHeight, metrics, Style::TextWrapStyle::Balance);
}

std::vector<TextLine> prettyWrapLine(const TextLine& source, float available, float fallbackHeight, const TextMeasurer& metrics) {
    return optimizedWrapLine(source, available, fallbackHeight, metrics, Style::TextWrapStyle::Pretty);
}

std::vector<TextLine> avoidShortLastLine(const TextLine& source, float available, float fallbackHeight, const TextMeasurer& metrics) {
    const std::vector<TextLine> greedy = wrapLine(source, available, fallbackHeight, metrics);
    if (greedy.size() < 2)
        return greedy;
    const std::vector<TextLine> candidate = prettyWrapLine(source, available, fallbackHeight, metrics);
    if (candidate.size() != greedy.size())
        return greedy;
    return lineSize(candidate.back(), fallbackHeight, metrics).x > lineSize(greedy.back(), fallbackHeight, metrics).x ? candidate : greedy;
}

TextLine visualRuns(const TextLine& line, Direction direction, const TextMeasurer& metrics) {
    if (line.size() < 2)
        return line;

    std::vector<FriBidiChar> logical;
    std::vector<std::pair<std::size_t, std::size_t>> ranges;
    ranges.reserve(line.size());
    for (const TextRun& run : line) {
        const std::size_t begin = logical.size();
        const LLWString wide = utf8str_to_wstring(run.value);
        for (llwchar character : wide)
            logical.push_back(static_cast<FriBidiChar>(character));
        ranges.emplace_back(begin, logical.size());
    }
    if (logical.empty() || logical.size() > static_cast<std::size_t>(std::numeric_limits<FriBidiStrIndex>::max()))
        return line;

    std::vector<FriBidiStrIndex> logicalToVisual(logical.size());
    std::vector<FriBidiLevel> levels(logical.size());
    FriBidiParType baseDirection = direction == Direction::RightToLeft ? FRIBIDI_PAR_RTL : FRIBIDI_PAR_LTR;
    if (!fribidi_log2vis(logical.data(), static_cast<FriBidiStrIndex>(logical.size()), &baseDirection, nullptr, logicalToVisual.data(),
            nullptr, levels.data()))
        return line;

    struct VisualRun {
        FriBidiStrIndex start = 0;
        TextRun run;
    };

    std::vector<VisualRun> segments;
    for (std::size_t runIndex = 0; runIndex < line.size(); ++runIndex) {
        const auto [runBegin, runEnd] = ranges[runIndex];
        std::size_t begin = runBegin;
        while (begin < runEnd) {
            std::size_t end = begin + 1;
            while (end < runEnd && levels[end] == levels[begin])
                ++end;

            FriBidiStrIndex visualStart = logicalToVisual[begin];
            LLWString wide;
            wide.reserve(end - begin);
            for (std::size_t offset = begin; offset < end; ++offset) {
                visualStart = std::min(visualStart, logicalToVisual[offset]);
                wide.push_back(static_cast<llwchar>(logical[offset]));
            }
            const std::string value = wstring_to_utf8str(wide);
            segments.push_back({
                visualStart,
                makeRun(value, line[runIndex].style, metrics),
            });
            begin = end;
        }
    }
    std::stable_sort(segments.begin(), segments.end(), [](const VisualRun& left, const VisualRun& right) {
        return left.start < right.start;
    });

    TextLine result;
    result.reserve(segments.size());
    for (VisualRun& segment : segments)
        result.push_back(std::move(segment.run));
    return result;
}

TextLine truncateLine(const TextLine& line, float available, float fallbackHeight, const Style::ComputedStyle& style,
    const TextMeasurer& metrics) {
    if (lineSize(line, fallbackHeight, metrics).x <= available)
        return line;
    if (style.textOverflow() == Style::TextOverflow::Clip)
        return line;
    const std::vector<TextChunk> clusters = characterClusters(line, metrics);
    if (clusters.empty())
        return {};

    const auto fits = [&](std::size_t prefixCount, const TextAtom* separator, std::size_t suffixBegin) {
        return lineSize(select(clusters, prefixCount, separator, suffixBegin, metrics), fallbackHeight, metrics).x <= available;
    };

    constexpr std::size_t kEllipsisSource = std::numeric_limits<std::size_t>::max();
    const TextAtom ellipsis {
        makeRun("\xE2\x80\xA6", style, metrics),
        kEllipsisSource,
        false,
    };
    if (!fits(0, &ellipsis, clusters.size()))
        return {};

    std::size_t prefixCount = 0;
    std::size_t suffixBegin = clusters.size();
    if (style.textOverflow() == Style::TextOverflow::Ellipsis) {
        std::size_t firstFailing = clusters.size() + 1;
        while (prefixCount + 1 < firstFailing) {
            const std::size_t candidate = prefixCount + (firstFailing - prefixCount) / 2;
            if (fits(candidate, &ellipsis, clusters.size()))
                prefixCount = candidate;
            else
                firstFailing = candidate;
        }
    } else {
        struct CenterSelection {
            std::size_t prefix;
            std::size_t suffix;
        };
        std::vector<CenterSelection> growth;
        growth.reserve(clusters.size() + 1);
        growth.push_back({prefixCount, suffixBegin});
        std::vector<float> clusterWidths;
        clusterWidths.reserve(clusters.size());
        for (const TextChunk& cluster : clusters)
            clusterWidths.push_back(lineSize(coalesce(cluster, metrics), fallbackHeight, metrics).x);
        float prefixAdvance = 0.f;
        float suffixAdvance = 0.f;
        while (prefixCount < suffixBegin) {
            const float nextPrefix = prefixAdvance + clusterWidths[prefixCount];
            const float nextSuffix = suffixAdvance + clusterWidths[suffixBegin - 1];
            if (nextPrefix <= nextSuffix) {
                prefixAdvance = nextPrefix;
                ++prefixCount;
            } else {
                suffixAdvance = nextSuffix;
                --suffixBegin;
            }
            growth.push_back({prefixCount, suffixBegin});
        }

        std::size_t selected = 0;
        std::size_t firstFailing = growth.size();
        while (selected + 1 < firstFailing) {
            const std::size_t candidate = selected + (firstFailing - selected) / 2;
            const CenterSelection state = growth[candidate];
            if (fits(state.prefix, &ellipsis, state.suffix))
                selected = candidate;
            else
                firstFailing = candidate;
        }
        prefixCount = growth[selected].prefix;
        suffixBegin = growth[selected].suffix;
    }
    return select(clusters, prefixCount, &ellipsis, suffixBegin, metrics);
}
} // namespace

float interRunSpacing(const TextRun& left, const TextRun& right, const TextMeasurer& metrics) {
    if (left.value.empty() || right.value.empty())
        return 0.f;
    const LLWString leftWide = utf8str_to_wstring(left.value);
    LLWString joined = leftWide;
    joined += utf8str_to_wstring(right.value);
    const std::vector<std::size_t> boundaries = graphemeBoundaries(joined);
    if (std::find(boundaries.begin(), boundaries.end(), leftWide.size()) == boundaries.end())
        return 0.f;
    return metrics.usedLetterSpacing(left.style);
}

LaidOutText layoutText(const std::vector<TextLine>& hardLines, const Style::ComputedStyle& style, const TextMeasurer& metrics,
    std::optional<float> availableWidth, bool visualOrder, bool applyOverflow) {
    LaidOutText result;
    const float fallbackHeight = metrics.measureText({}, style).y;
    for (const TextLine& hardLine : hardLines) {
        std::vector<TextLine> visualLines;
        if (availableWidth && style.textWrapMode() == Style::TextWrapMode::Wrap)
            if (style.textWrapStyle() == Style::TextWrapStyle::Balance)
                visualLines = balancedWrapLine(hardLine, *availableWidth, fallbackHeight, metrics);
            else if (style.textWrapStyle() == Style::TextWrapStyle::Pretty)
                visualLines = prettyWrapLine(hardLine, *availableWidth, fallbackHeight, metrics);
            else if (style.textWrapStyle() == Style::TextWrapStyle::AvoidShortLastLine)
                visualLines = avoidShortLastLine(hardLine, *availableWidth, fallbackHeight, metrics);
            else
                visualLines = wrapLine(hardLine, *availableWidth, fallbackHeight, metrics);
        else
            visualLines.push_back(hardLine);

        for (TextLine& line : visualLines) {
            if (availableWidth && applyOverflow && style.textWrapMode() == Style::TextWrapMode::NoWrap
                && style.overflowX() == Style::Overflow::Hidden)
                line = truncateLine(line, *availableWidth, fallbackHeight, style, metrics);
            if (visualOrder)
                line = visualRuns(line, style.direction, metrics);

            const Vec2 size = lineSize(line, fallbackHeight, metrics);
            result.size.x = std::max(result.size.x, size.x);
            result.size.y += size.y;
            result.lines.push_back({std::move(line), size});
        }
    }
    return result;
}
} // namespace Core::Layout::detail

namespace Core::Layout {
namespace {
using detail::TextLine;
using detail::TextRun;

bool isCollapsibleWhitespace(char character) {
    return character == ' ' || character == '\t' || character == '\n' || character == '\f' || character == '\r';
}

std::string collapseWhitespace(const std::string& text) {
    std::string result;
    result.reserve(text.size());
    bool whitespace = false;
    for (const char character : text) {
        if (isCollapsibleWhitespace(character)) {
            whitespace = true;
            continue;
        }
        if (whitespace)
            result.push_back(' ');
        result.push_back(character);
        whitespace = false;
    }
    if (whitespace)
        result.push_back(' ');
    return result;
}

std::vector<TextLine> layoutLines(const std::string& text, const Style::ComputedStyle& style, const TextMeasurer& metrics) {
    std::vector<TextLine> lines(1);
    const std::string value = collapseWhitespace(text);
    if (!value.empty())
        lines.front().push_back({value, style, metrics.measureText(value, style)});
    return lines;
}

float alignedOffset(float available, float occupied, Style::TextAlign alignment, Direction direction) {
    if (alignment == Style::TextAlign::Center)
        return (available - occupied) * .5f;
    if (alignment == Style::TextAlign::Right || alignment == Style::TextAlign::End)
        return available - occupied;
    if (alignment == Style::TextAlign::MatchParent)
        return direction == Direction::RightToLeft ? available - occupied : 0.f;
    return 0.f;
}

std::size_t whitespaceCount(const TextRun& run) {
    const LLWString value = utf8str_to_wstring(run.value);
    return static_cast<std::size_t>(std::count_if(value.begin(), value.end(), [](llwchar character) {
        return std::iswspace(static_cast<wint_t>(character)) != 0;
    }));
}

void mixStyleValue(std::size_t& hash, std::size_t value) {
    hash ^= value + static_cast<std::size_t>(0x9e3779b9) + (hash << 6) + (hash >> 2);
}

void mixStyleValue(std::size_t& hash, float value) { mixStyleValue(hash, std::hash<float> {}(value)); }

template<typename Constraint> void mixLength(std::size_t& hash, const Style::LengthValue<Constraint, float>& value) {
    mixStyleValue(hash, value.pixels);
    mixStyleValue(hash, value.percent);
}

std::size_t textStyleFingerprint(const Style::ComputedStyle& style) {
    std::size_t hash = 0;
    for (const Style::FontFamily& family : style.fontFamily()) {
        mixStyleValue(hash, family.index());
        if (const auto* name = std::get_if<std::string>(&family))
            mixStyleValue(hash, std::hash<std::string> {}(*name));
        else
            mixStyleValue(hash, static_cast<std::size_t>(std::get<Style::GenericFontFamily>(family)));
    }
    mixStyleValue(hash, style.fontSize());
    mixStyleValue(hash, style.fontWeight().value);
    mixStyleValue(hash, style.fontWidth().percentage);
    mixStyleValue(hash, static_cast<std::size_t>(style.fontStyle()));
    const auto& lineHeightValue = style.lineHeight().mValue;
    mixStyleValue(hash, lineHeightValue.index());
    if (const auto* number = std::get_if<Style::LineHeight::Number>(&lineHeightValue))
        mixStyleValue(hash, number->value);
    if (const auto* length = std::get_if<Style::LineHeight::Length>(&lineHeightValue)) {
        mixStyleValue(hash, length->pixels);
        mixStyleValue(hash, length->percent);
    }
    mixLength(hash, style.letterSpacing());
    mixLength(hash, style.wordSpacing());
    return hash;
}

std::size_t textLayoutFingerprint(const Style::ComputedStyle& style, bool visualOrder, bool applyOverflow) {
    std::size_t hash = 0;
    mixStyleValue(hash, static_cast<std::size_t>(style.textWrapMode()));
    mixStyleValue(hash, static_cast<std::size_t>(style.textWrapStyle()));
    if (applyOverflow) {
        mixStyleValue(hash, static_cast<std::size_t>(style.textOverflow()));
        mixStyleValue(hash, static_cast<std::size_t>(style.overflowX()));
    }
    if (visualOrder)
        mixStyleValue(hash, static_cast<std::size_t>(style.direction));
    return hash;
}
} // namespace

void TextLayout::setText(std::string text) {
    mText = std::move(text);
    ++mContentGeneration;
}

Vec2 TextLayout::measure(const TextMeasurer& metrics, const Style::ComputedStyle& style, const CSS::StyleSheet& styleSheet,
    const Element& owner, std::optional<float> resolvedWidth) const {
    std::optional<float> availableWidth;
    if (resolvedWidth)
        availableWidth = std::max(0.f, *resolvedWidth - paddingPixels(style).horizontal());
    else if (!style.width().isAuto() && !style.width().isPercentage() && !style.width().isIntrinsic())
        availableWidth = std::max(0.f, style.width().pixels() - paddingPixels(style).horizontal());
    return cachedLayout(metrics, style, &styleSheet, owner, availableWidth, false, false).size;
}

void TextLayout::preparePaint(const TextMeasurer& metrics, const Style::ComputedStyle& style, const CSS::StyleSheet& styleSheet,
    const Element& owner, float availableWidth) const {
    (void)cachedLayout(metrics, style, &styleSheet, owner, availableWidth, true, true);
}

void TextLayout::paint(PaintContext& context, const Rect& rect, const Style::ComputedStyle& style, const CSS::StyleSheet* styleSheet,
    const Element& owner) const {
    const TextMeasurer& metrics = context.textMetrics();
    const detail::LaidOutText& layout = cachedLayout(metrics, style, styleSheet, owner, rect.w, true, true);
    const TextPaintStyle paintStyle {style.color().resolvedColor(),
        style.textDecorationPropagation == Style::TextDecoration::NoneValue ? style.textDecoration() : style.textDecorationPropagation,
        style.textAlign(), style.direction};
    paintLayout(context, rect, paintStyle, layout, metrics);
}

void TextLayout::paintPrepared(PaintContext& context, const Rect& rect, const Style::ComputedStyle& layoutStyle,
    const TextPaintStyle& paintStyle, const CSS::StyleSheet* styleSheet, const Element& owner) const {
    const TextMeasurer& metrics = context.textMetrics();
    const bool widthMatches = mCachedLayoutWidthSet && mCachedLayoutWidth == rect.w;
    const std::uint64_t styleSheetGeneration = styleSheet ? styleSheet->generation() : 0;
    const bool matches = mCachedLayoutValid && mCachedContentGeneration == mContentGeneration && mCachedMetrics == &metrics
        && mCachedMetricsGeneration == metrics.generation() && mCachedStyleSheet == styleSheet
        && mCachedStyleSheetGeneration == styleSheetGeneration && mCachedOwner == &owner && widthMatches && mCachedLayoutVisualOrder
        && mCachedLayoutOverflow && mCachedLayoutStyleFingerprint == textLayoutFingerprint(layoutStyle, true, true)
        && mCachedStyleFingerprint == textStyleFingerprint(layoutStyle);
    if (matches) {
        paintLayout(context, rect, paintStyle, mCachedLayout, *mCachedMetrics);
        return;
    }
    const detail::LaidOutText& layout = cachedLayout(metrics, layoutStyle, styleSheet, owner, rect.w, true, true);
    paintLayout(context, rect, paintStyle, layout, metrics);
}

void TextLayout::paintLayout(PaintContext& context, const Rect& rect, const TextPaintStyle& style, const detail::LaidOutText& layout,
    const TextMeasurer& metrics) const {
    float y = rect.top();
    for (std::size_t lineIndex = 0; lineIndex < layout.lines.size(); ++lineIndex) {
        const detail::LaidOutTextLine& line = layout.lines[lineIndex];
        y -= line.size.y;
        const bool justify = style.textAlign == Style::TextAlign::Justify || style.textAlign == Style::TextAlign::JustifyAll;
        const bool justifyLine = justify && (style.textAlign == Style::TextAlign::JustifyAll || lineIndex + 1 < layout.lines.size());
        std::size_t opportunities = 0;
        if (justifyLine)
            for (const TextRun& run : line.runs)
                opportunities += whitespaceCount(run);
        const float extra = justifyLine && opportunities ? std::max(0.f, rect.w - line.size.x) / static_cast<float>(opportunities) : 0.f;
        float x = rect.x + alignedOffset(rect.w, line.size.x, style.textAlign, style.direction);
        for (std::size_t runIndex = 0; runIndex < line.runs.size(); ++runIndex) {
            const TextRun& run = line.runs[runIndex];
            const auto paintRun = [&](const std::string& value, float width) {
                Style::ComputedStyle runStyle = run.style;
                runStyle.setColor(style.color);
                runStyle.setTextDecoration(style.textDecoration);
                runStyle.setTextAlign(Style::TextAlign::Left);
                context.paintText(value, {x, y, width, line.size.y}, runStyle);
                x += width;
            };
            if (!justifyLine || whitespaceCount(run) == 0)
                paintRun(run.value, run.size.x);
            else {
                const LLWString wide = utf8str_to_wstring(run.value);
                for (std::size_t begin = 0; begin < wide.size();) {
                    const bool whitespace = std::iswspace(static_cast<wint_t>(wide[begin])) != 0;
                    std::size_t end = begin + 1;
                    while (end < wide.size() && (std::iswspace(static_cast<wint_t>(wide[end])) != 0) == whitespace)
                        ++end;
                    const std::string value = wstring_to_utf8str(wide.substr(begin, end - begin));
                    paintRun(value, metrics.measureText(value, run.style).x);
                    if (whitespace)
                        x += extra * static_cast<float>(end - begin);
                    begin = end;
                }
            }
            if (runIndex + 1 < line.runs.size())
                x += interRunSpacing(run, line.runs[runIndex + 1], metrics);
        }
    }
}

const std::vector<detail::TextLine>& TextLayout::cachedLines(const TextMeasurer& metrics, const Style::ComputedStyle& style,
    const CSS::StyleSheet* styleSheet, const Element& owner) const {
    const std::size_t fingerprint = textStyleFingerprint(style);
    const std::uint64_t styleSheetGeneration = styleSheet ? styleSheet->generation() : 0;
    const std::uint64_t metricsGeneration = metrics.generation();
    if (mCachedContentGeneration == mContentGeneration && mCachedMetrics == &metrics && mCachedMetricsGeneration == metricsGeneration
        && mCachedStyleSheet == styleSheet && mCachedStyleSheetGeneration == styleSheetGeneration && mCachedOwner == &owner
        && mCachedStyleFingerprint == fingerprint)
        return mCachedLines;

    mCachedLines = layoutLines(mText, style, metrics);
    mCachedLayoutValid = false;
    mCachedContentGeneration = mContentGeneration;
    mCachedMetrics = &metrics;
    mCachedMetricsGeneration = metricsGeneration;
    mCachedStyleSheet = styleSheet;
    mCachedStyleSheetGeneration = styleSheetGeneration;
    mCachedOwner = &owner;
    mCachedStyleFingerprint = fingerprint;
    return mCachedLines;
}

const detail::LaidOutText& TextLayout::cachedLayout(const TextMeasurer& metrics, const Style::ComputedStyle& style,
    const CSS::StyleSheet* styleSheet, const Element& owner, std::optional<float> availableWidth, bool visualOrder,
    bool applyOverflow) const {
    const std::vector<detail::TextLine>& lines = cachedLines(metrics, style, styleSheet, owner);
    const std::size_t layoutFingerprint = textLayoutFingerprint(style, visualOrder, applyOverflow);
    const bool widthMatches =
        mCachedLayoutWidthSet == availableWidth.has_value() && (!availableWidth || mCachedLayoutWidth == *availableWidth);
    if (!mCachedLayoutValid || !widthMatches || mCachedLayoutVisualOrder != visualOrder || mCachedLayoutOverflow != applyOverflow
        || mCachedLayoutStyleFingerprint != layoutFingerprint) {
        mCachedLayout = detail::layoutText(lines, style, metrics, availableWidth, visualOrder, applyOverflow);
        mCachedLayoutWidthSet = availableWidth.has_value();
        mCachedLayoutWidth = availableWidth.value_or(0.f);
        mCachedLayoutVisualOrder = visualOrder;
        mCachedLayoutOverflow = applyOverflow;
        mCachedLayoutStyleFingerprint = layoutFingerprint;
        mCachedLayoutValid = true;
    }
    return mCachedLayout;
}
} // namespace Core::Layout
