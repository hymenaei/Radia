/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <benchmark/benchmark.h>
#include <cstddef>
#include <memory>
#include <string>
#include "css/stylesheet.h"
#include "dom/element.h"
#include "dom/elementinternal.h"
#include "html/button.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "layout/engine.h"
#include "style/stylepass.h"
#include "text/metrics.h"

namespace {
using radia::ui::Element;
using radia::ui::fixedTextMetrics;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::LayoutDirection;
using radia::ui::LayoutEngine;
using radia::ui::LayoutStatistics;
using radia::ui::Rect;
using radia::ui::StylePass;
using radia::ui::StyleSheet;
using radia::ui::Visibility;
using radia::ui::detail::appendText;
using radia::ui::detail::makeElement;

std::unique_ptr<Element> makeParagraph(std::string text) {
    auto paragraph = makeElement<Element>("p");
    paragraph->textContent(std::move(text));
    return paragraph;
}

enum class LayoutCase {
    FlatColumn,
    FlatRow,
    BalancedTree,
    FlexRow,
    Normal,
    ShortLabels,
    WrappedText,
    CompositeControls,
    HiddenLabels,
    CollapsedLabels
};

struct LayoutFixture {
    std::unique_ptr<HTMLPanelElement> root;
    StyleSheet styleSheet;
};

void addFlatLabels(HTMLPanelElement& root, std::size_t nodeCount, bool withText, Visibility specialVisibility = Visibility::Visible) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        auto label = makeElement<HTMLLabelElement>(withText ? "Item " + std::to_string(index) : std::string());
        if (specialVisibility != Visibility::Visible && index % 4 == 0) label->setVisibility(specialVisibility);
        root.append(std::move(label));
    }
}

void addBalancedChildren(HTMLPanelElement& parent, std::size_t nodeCount, std::size_t depth) {
    if (nodeCount == 0) return;

    const std::size_t branchCount = depth % 2 == 0 ? 2 : 3;
    if (nodeCount <= branchCount) {
        for (std::size_t index = 0; index < nodeCount; ++index) parent.append(makeElement<HTMLLabelElement>());
        return;
    }

    const std::size_t descendantCount = nodeCount - branchCount;
    const std::size_t descendantsPerChild = descendantCount / branchCount;
    const std::size_t extraDescendants = descendantCount % branchCount;
    for (std::size_t branch = 0; branch < branchCount; ++branch) {
        auto child = makeElement<HTMLPanelElement>();
        HTMLPanelElement* childPointer = child.get();
        parent.append(std::move(child));
        const std::size_t childNodeCount = descendantsPerChild + (branch < extraDescendants ? 1 : 0);
        addBalancedChildren(*childPointer, childNodeCount, depth + 1);
    }
}

std::size_t countElements(const Element& element) {
    std::size_t count = 1;
    for (const Element* child : element.children()) count += countElements(*child);
    return count;
}

void addExplicitLabels(HTMLPanelElement& root, std::size_t nodeCount) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        auto label = makeElement<HTMLLabelElement>();
        const float x = static_cast<float>(index % 32) * 18.f;
        const float y = static_cast<float>(index / 32) * 14.f;
        label->setRect({x, y, 14.f, 10.f});
        root.append(std::move(label));
    }
}

void addCompositeControls(HTMLPanelElement& root, std::size_t nodeCount) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        switch (index % 3) {
            case 0: {
                auto button = makeElement<HTMLButtonElement>();
                auto icon = makeElement<Element>("i");
                icon->addClass("i-search");
                button->append(std::move(icon));
                appendText(*button, "Apply");
                root.append(std::move(button));
                break;
            }
            case 1: {
                auto control = makeElement<HTMLInputElement>();
                control->type("checkbox").switchMode(true);
                control->checked(index % 2 == 0);
                root.append(std::move(control));
                break;
            }
            default: root.append(makeElement<HTMLLabelElement>("Status")); break;
        }
    }
}

bool makeFixture(LayoutFixture& fixture, LayoutCase layoutCase, std::size_t nodeCount, benchmark::State& state) {
    fixture.root = makeElement<HTMLPanelElement>();

    std::string styleSource;
    Rect rootRect{0.f, 0.f, 0.f, 0.f};
    switch (layoutCase) {
        case LayoutCase::FlatColumn:
        case LayoutCase::HiddenLabels:
        case LayoutCase::CollapsedLabels: {
            styleSource = "panel { display: flex; flex-direction: column; gap: 2px; } label { width: 120px; height: 10px; }";
            Visibility specialVisibility = Visibility::Visible;
            if (layoutCase == LayoutCase::HiddenLabels) specialVisibility = Visibility::Hidden;
            else if (layoutCase == LayoutCase::CollapsedLabels) specialVisibility = Visibility::Collapse;
            addFlatLabels(*fixture.root, nodeCount, false, specialVisibility);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        }
        case LayoutCase::FlatRow:
            styleSource = "panel { display: flex; flex-direction: row; gap: 1px; } label { width: 10px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f), 24.f};
            break;
        case LayoutCase::BalancedTree: {
            styleSource = "panel { display: flex; flex-direction: row; gap: 1px; } label { width: 10px; height: 10px; }";
            addBalancedChildren(*fixture.root, nodeCount, 0);
            rootRect = {0.f, 0.f, 800.f, 600.f};
            break;
        }
        case LayoutCase::FlexRow:
            styleSource = "panel { display: flex; flex-direction: row; gap: 2px; } label { flex: 1; min-width: 0px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, 1200.f, 24.f};
            break;
        case LayoutCase::Normal:
            styleSource = "panel { display: block; } label { width: 14px; height: 10px; }";
            addExplicitLabels(*fixture.root, nodeCount);
            rootRect = {0.f, 0.f, 600.f, std::max(200.f, static_cast<float>((nodeCount + 31) / 32) * 14.f + 20.f)};
            break;
        case LayoutCase::ShortLabels:
            styleSource =
                "panel { display: flex; flex-direction: column; gap: 2px; } label { width: 180px; height: 18px; font-size: 12px; line-height: 18px; }";
            addFlatLabels(*fixture.root, nodeCount, true);
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 20.f)};
            break;
        case LayoutCase::WrappedText:
            styleSource =
                "panel { display: flex; flex-direction: column; gap: 2px; } p { width: 48px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
            for (std::size_t index = 0; index < nodeCount; ++index) fixture.root->append(makeParagraph("alpha beta gamma delta"));
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 42.f)};
            break;
        case LayoutCase::CompositeControls:
            styleSource =
                "panel { display: flex; flex-direction: column; gap: 2px; } "
                "button { width: 160px; height: 24px; padding: 4px; gap: 4px; display: flex; flex-direction: row; } "
                "button > i { size: 16px; } input { width: 64px; height: 24px; } "
                "input { display: flex; flex-direction: row; } input::slider-track { width: 100%; min-width: 0; align-self: stretch; } "
                "input::slider-thumb { order: -1; size: 18px; } input:checked::slider-thumb { order: 1; } label { width: 160px; height: 18px; }";
            addCompositeControls(*fixture.root, nodeCount);
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 28.f)};
            break;
    }

    if (styleSource.empty()) {
        state.SkipWithError("No fixture defined for this layout case.");
        return false;
    }

    if (layoutCase == LayoutCase::BalancedTree && countElements(*fixture.root) != nodeCount + 1) {
        state.SkipWithError("Balanced tree fixture did not create the requested element count.");
        return false;
    }

    fixture.root->setRect(rootRect);
    const auto loadResult = fixture.styleSheet.loadRadia(styleSource, "engine_benchmark.css");
    if (!loadResult.ok()) {
        state.SkipWithError(loadResult.errors.empty() ? "Failed to load benchmark stylesheet." : loadResult.errors.front().formatted());
        return false;
    }
    return true;
}

LayoutStatistics runLayout(Element& root, StylePass& styles) {
    const StylePass::TraversalScope traversal = styles.enterTraversal();
    return LayoutEngine::layout(root, styles);
}

bool requireRelayout(benchmark::State& state, const LayoutStatistics& statistics, const char* message) {
    if (statistics.measuredNodes != 0 && statistics.arrangedNodes != 0) return true;
    state.SkipWithError(message);
    return false;
}

bool requireCachedLayout(benchmark::State& state, const LayoutStatistics& statistics, const char* message) {
    if (statistics.measuredNodes == 0 && statistics.arrangedNodes == 0 && statistics.skippedNodes != 0) return true;
    state.SkipWithError(message);
    return false;
}

template<typename Prepare> void timedLayout(benchmark::State& state, Element& root, Prepare&& prepare) {
    state.PauseTiming();
    {
        StylePass& styles = prepare();
        const StylePass::TraversalScope traversal = styles.enterTraversal();
        state.ResumeTiming();
        LayoutEngine::layout(root, styles);
        state.PauseTiming();
    }
    state.ResumeTiming();
}

void timedLayout(benchmark::State& state, Element& root, StylePass& styles) {
    timedLayout(state, root, [&styles]() -> StylePass& { return styles; });
}

void BM_ComputeLayout(benchmark::State& state, LayoutCase layoutCase) {
    // Measure CPU layout cost after a warmed fixture changes size. Fixture
    // construction, stylesheet parsing, style-pass construction, and the
    // rectangle mutation are excluded from the timed layout call.
    LayoutFixture fixture;
    if (!makeFixture(fixture, layoutCase, static_cast<std::size_t>(state.range(0)), state)) return;
    const auto& textMetrics = fixedTextMetrics();
    const Rect baseRect = fixture.root->rect();
    const Rect firstResize{baseRect.x, baseRect.y, baseRect.w + 1.f, baseRect.h};
    const Rect secondResize{baseRect.x, baseRect.y, baseRect.w + 2.f, baseRect.h};
    StylePass styles(fixture.styleSheet, textMetrics, LayoutDirection::LeftToRight);

    runLayout(*fixture.root, styles);

    LayoutStatistics resizeProbe;
    {
        fixture.root->setRect(firstResize);
        resizeProbe = runLayout(*fixture.root, styles);
    }
    if (!requireRelayout(state, resizeProbe, "Resize benchmark did not invalidate layout state.")) return;

    const LayoutStatistics cacheProbe = runLayout(*fixture.root, styles);
    if (!requireCachedLayout(state, cacheProbe, "Resize benchmark did not reuse the cached layout state.")) return;

    fixture.root->setRect(baseRect);
    runLayout(*fixture.root, styles);

    std::size_t iteration = 0;
    for (auto _ : state) {
        timedLayout(state, *fixture.root, [&]() -> StylePass& {
            fixture.root->setRect(iteration++ % 2 == 0 ? firstResize : secondResize);
            return styles;
        });
    }
}

void BM_CachedLayout(benchmark::State& state, LayoutCase layoutCase) {
    // Measure the CPU cost of resolving a layout tree that has already reached
    // its steady state. The first layout warms the cache and is not measured.
    LayoutFixture fixture;
    if (!makeFixture(fixture, layoutCase, static_cast<std::size_t>(state.range(0)), state)) return;
    const auto& textMetrics = fixedTextMetrics();
    StylePass styles(fixture.styleSheet, textMetrics, LayoutDirection::LeftToRight);

    runLayout(*fixture.root, styles);
    const LayoutStatistics cacheProbe = runLayout(*fixture.root, styles);
    if (!requireCachedLayout(state, cacheProbe, "Steady-state benchmark did not reach the cached layout path.")) return;

    for (auto _ : state) timedLayout(state, *fixture.root, styles);
}

void BM_DirectionChange(benchmark::State& state) {
    // Measure CPU layout cost when direction alternates between iterations.
    // Each direction keeps a warmed style pass, so style-pass allocation and
    // selector resolution do not bias the layout timing.
    LayoutFixture fixture;
    if (!makeFixture(fixture, LayoutCase::FlatRow, static_cast<std::size_t>(state.range(0)), state)) return;
    const auto& textMetrics = fixedTextMetrics();
    StylePass leftToRightStyles(fixture.styleSheet, textMetrics, LayoutDirection::LeftToRight);
    StylePass rightToLeftStyles(fixture.styleSheet, textMetrics, LayoutDirection::RightToLeft);

    runLayout(*fixture.root, leftToRightStyles);
    const LayoutStatistics relayoutProbe = runLayout(*fixture.root, rightToLeftStyles);
    if (!requireRelayout(state, relayoutProbe, "Direction benchmark did not exercise a full relayout.")) return;
    runLayout(*fixture.root, leftToRightStyles);

    bool rightToLeft = false;
    for (auto _ : state) {
        timedLayout(state, *fixture.root, [&]() -> StylePass& {
            rightToLeft = !rightToLeft;
            return rightToLeft ? rightToLeftStyles : leftToRightStyles;
        });
    }
}

void configureLayoutBenchmark(benchmark::Benchmark* layoutBenchmark) {
    layoutBenchmark->Unit(benchmark::kMicrosecond)->RangeMultiplier(8)->Range(8, 4096);
}

#define BENCHMARK_CASE(functionName, caseName)                                                                                                       \
    BENCHMARK_CAPTURE(functionName, caseName, LayoutCase::caseName)->Name(#functionName "::" #caseName)->Apply(configureLayoutBenchmark)

BENCHMARK_CASE(BM_ComputeLayout, FlatColumn);
BENCHMARK_CASE(BM_ComputeLayout, FlatRow);
BENCHMARK_CASE(BM_ComputeLayout, BalancedTree);
BENCHMARK_CASE(BM_ComputeLayout, FlexRow);
BENCHMARK_CASE(BM_ComputeLayout, Normal);
BENCHMARK_CASE(BM_ComputeLayout, ShortLabels);
BENCHMARK_CASE(BM_ComputeLayout, WrappedText);
BENCHMARK_CASE(BM_ComputeLayout, CompositeControls);
BENCHMARK_CASE(BM_ComputeLayout, HiddenLabels);
BENCHMARK_CASE(BM_ComputeLayout, CollapsedLabels);
BENCHMARK_CASE(BM_CachedLayout, FlatColumn);
BENCHMARK_CASE(BM_CachedLayout, WrappedText);
BENCHMARK_CASE(BM_CachedLayout, CompositeControls);
#undef BENCHMARK_CASE
BENCHMARK(BM_DirectionChange)->Name("BM_DirectionChange")->Apply(configureLayoutBenchmark);
} // namespace
