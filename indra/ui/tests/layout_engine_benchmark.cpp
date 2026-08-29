/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <algorithm>
#include <array>
#include <benchmark/benchmark.h>
#include <cstddef>
#include <cstdlib>
#include <memory>
#include <string>
#include "elements/button.h"
#include "elements/element.h"
#include "elements/icon.h"
#include "elements/input.h"
#include "elements/label.h"
#include "elements/panel.h"
#include "layout/engine.h"
#include "style/stylesheet.h"
#include "text/metrics.h"

namespace {
using radia::ui::ButtonElement;
using radia::ui::Element;
using radia::ui::fixedTextMetrics;
using radia::ui::IconElement;
using radia::ui::InputElement;
using radia::ui::LabelElement;
using radia::ui::LayoutDirection;
using radia::ui::LayoutStatistics;
using radia::ui::layoutTree;
using radia::ui::PanelElement;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Visibility;

std::unique_ptr<Element> makeParagraph(std::string text) {
    auto paragraph = std::make_unique<Element>("p");
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
    CollapsedLabels,
    RightToLeftRow
};

struct LayoutFixture {
    std::unique_ptr<PanelElement> root;
    StyleSheet styleSheet;
    LayoutDirection direction = LayoutDirection::LeftToRight;
};

constexpr std::array<int, 5> kScaleNodeCounts = {0, 1, 10, 100, 1000};
constexpr std::array<int, 2> kRepresentativeNodeCounts = {100, 1000};
constexpr std::array<int, 1> kStateNodeCounts = {1000};
constexpr std::array<int, 1> kCacheNodeCounts = {1000};
constexpr std::array<int, 2> kDirectionNodeCounts = {100, 1000};

void addFlatLabels(PanelElement& root, std::size_t nodeCount, bool withText, Visibility specialVisibility = Visibility::Visible) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        auto label = std::make_unique<LabelElement>(withText ? "Item " + std::to_string(index) : std::string());
        if (specialVisibility != Visibility::Visible && index % 4 == 0) label->setVisibility(specialVisibility);
        root.append(std::move(label));
    }
}

void addBalancedChildren(PanelElement& parent, std::size_t& remaining, std::size_t depth) {
    if (remaining == 0) return;

    const std::size_t branchCount = depth % 2 == 0 ? 2 : 3;
    for (std::size_t branch = 0; branch < branchCount && remaining > 0; ++branch) {
        if (remaining > 1 && depth < 8) {
            auto child = std::make_unique<PanelElement>();
            PanelElement* childPointer = child.get();
            parent.append(std::move(child));
            --remaining;
            addBalancedChildren(*childPointer, remaining, depth + 1);
        } else {
            parent.append(std::make_unique<LabelElement>());
            --remaining;
        }
    }
}

void addExplicitLabels(PanelElement& root, std::size_t nodeCount) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        auto label = std::make_unique<LabelElement>();
        const float x = static_cast<float>(index % 32) * 18.f;
        const float y = static_cast<float>(index / 32) * 14.f;
        label->setRect({x, y, 14.f, 10.f});
        root.append(std::move(label));
    }
}

void addCompositeControls(PanelElement& root, std::size_t nodeCount) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        switch (index % 3) {
            case 0: {
                auto button = std::make_unique<ButtonElement>();
                auto icon = std::make_unique<IconElement>("search");
                button->append(std::move(icon));
                radia::ui::detail::appendText(*button, "Apply");
                root.append(std::move(button));
                break;
            }
            case 1: {
                auto control = std::make_unique<InputElement>();
                control->type("checkbox").switchMode(true);
                control->checked(index % 2 == 0);
                root.append(std::move(control));
                break;
            }
            default: root.append(std::make_unique<LabelElement>("Status")); break;
        }
    }
}

LayoutFixture makeFixture(LayoutCase layoutCase, std::size_t nodeCount) {
    LayoutFixture fixture;
    fixture.root = std::make_unique<PanelElement>();

    std::string styleSource;
    Rect rootRect;
    switch (layoutCase) {
        case LayoutCase::FlatColumn:
            styleSource = "panel { display: flex; flex-direction: column; gap: 2px; } label { width: 120px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        case LayoutCase::FlatRow:
            styleSource = "panel { display: flex; flex-direction: row; gap: 1px; } label { width: 10px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f), 24.f};
            break;
        case LayoutCase::BalancedTree: {
            styleSource = "panel { display: flex; flex-direction: row; gap: 1px; } label { width: 10px; height: 10px; }";
            std::size_t remaining = nodeCount;
            addBalancedChildren(*fixture.root, remaining, 0);
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
            styleSource = "panel { display: flex; flex-direction: column; gap: 2px; } "
                          "button { width: 160px; height: 24px; padding: 4px; gap: 4px; display: flex; flex-direction: row; } "
                          "button > icon { size: 16px; } input { width: 64px; height: 24px; } "
                          "input { display: flex; flex-direction: row; } input::track { width: 100%; min-width: 0; align-self: stretch; } "
                          "input::thumb { order: -1; size: 18px; } input:checked::thumb { order: 1; } label { width: 160px; height: 18px; }";
            addCompositeControls(*fixture.root, nodeCount);
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 28.f)};
            break;
        case LayoutCase::HiddenLabels:
            styleSource = "panel { display: flex; flex-direction: column; gap: 2px; } label { width: 120px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false, Visibility::Hidden);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        case LayoutCase::CollapsedLabels:
            styleSource = "panel { display: flex; flex-direction: column; gap: 2px; } label { width: 120px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false, Visibility::Collapse);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        case LayoutCase::RightToLeftRow:
            styleSource = "panel { display: flex; flex-direction: row; gap: 1px; } label { width: 10px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            fixture.direction = LayoutDirection::RightToLeft;
            rootRect = {0.f, 0.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f), 24.f};
            break;
    }

    fixture.root->setRect(rootRect);
    if (!fixture.styleSheet.loadRadia(styleSource, "engine_benchmark.css").ok()) std::abort();
    return fixture;
}

struct StatisticsTotals {
    std::size_t measuredNodes = 0;
    std::size_t constrainedRemeasures = 0;
    std::size_t arrangedNodes = 0;
    std::size_t cacheSkips = 0;

    void add(const LayoutStatistics& statistics) {
        measuredNodes += statistics.measuredNodes;
        constrainedRemeasures += statistics.constrainedRemeasures;
        arrangedNodes += statistics.arrangedNodes;
        cacheSkips += statistics.skippedNodes;
    }

    void publish(benchmark::State& state) const {
        state.counters["MeasureCalls"] = benchmark::Counter(static_cast<double>(measuredNodes), benchmark::Counter::kAvgIterations);
        state.counters["ConstrainedMeasureCalls"] =
            benchmark::Counter(static_cast<double>(constrainedRemeasures), benchmark::Counter::kAvgIterations);
        state.counters["ArrangeCalls"] = benchmark::Counter(static_cast<double>(arrangedNodes), benchmark::Counter::kAvgIterations);
        state.counters["CacheReuseEvents"] = benchmark::Counter(static_cast<double>(cacheSkips), benchmark::Counter::kAvgIterations);
    }
};

void BM_Layout_RelayoutAfterResize(benchmark::State& state, LayoutCase layoutCase) {
    // Measure CPU layout cost after a warmed fixture changes size. Fixture
    // construction, stylesheet parsing, and the initial layout are excluded;
    // the rectangle mutation is scenario setup rather than layout work.
    LayoutFixture fixture = makeFixture(layoutCase, static_cast<std::size_t>(state.range(0)));
    const auto& textMetrics = fixedTextMetrics();
    const Rect baseRect = fixture.root->rect();

    LayoutStatistics warmup = layoutTree(*fixture.root, fixture.styleSheet, textMetrics, fixture.direction);
    benchmark::DoNotOptimize(warmup.measuredNodes);

    StatisticsTotals totals;
    std::size_t iteration = 0;
    for (auto _ : state) {
        const float width = baseRect.w + (iteration++ % 2 == 0 ? 1.f : 2.f);
        state.PauseTiming();
        fixture.root->setRect({baseRect.x, baseRect.y, width, baseRect.h});
        state.ResumeTiming();

        LayoutStatistics statistics = layoutTree(*fixture.root, fixture.styleSheet, textMetrics, fixture.direction);
        totals.add(statistics);
    }
    totals.publish(state);
}

void BM_Layout_CachedLayout(benchmark::State& state, LayoutCase layoutCase) {
    // Measure the CPU cost of resolving a layout tree that has already reached
    // its steady state. The first layout warms the cache and is not measured.
    LayoutFixture fixture = makeFixture(layoutCase, static_cast<std::size_t>(state.range(0)));
    const auto& textMetrics = fixedTextMetrics();
    LayoutStatistics warmup = layoutTree(*fixture.root, fixture.styleSheet, textMetrics, fixture.direction);
    benchmark::DoNotOptimize(warmup.measuredNodes);

    StatisticsTotals totals;
    for (auto _ : state) {
        LayoutStatistics statistics = layoutTree(*fixture.root, fixture.styleSheet, textMetrics, fixture.direction);
        totals.add(statistics);
    }
    totals.publish(state);
}

void BM_Layout_RelayoutAfterDirectionChange(benchmark::State& state, LayoutCase layoutCase) {
    // Measure CPU layout cost when direction alternates between iterations.
    // The direction transition is part of the workload represented by this
    // family, while fixture construction and the initial layout are excluded.
    LayoutFixture fixture = makeFixture(layoutCase, static_cast<std::size_t>(state.range(0)));
    const auto& textMetrics = fixedTextMetrics();
    LayoutStatistics warmup = layoutTree(*fixture.root, fixture.styleSheet, textMetrics, LayoutDirection::LeftToRight);
    benchmark::DoNotOptimize(warmup.measuredNodes);

    StatisticsTotals totals;
    bool rightToLeft = false;
    for (auto _ : state) {
        rightToLeft = !rightToLeft;
        const auto direction = rightToLeft ? LayoutDirection::RightToLeft : LayoutDirection::LeftToRight;
        LayoutStatistics statistics = layoutTree(*fixture.root, fixture.styleSheet, textMetrics, direction);
        totals.add(statistics);
    }
    totals.publish(state);
}

void addScaleCases(benchmark::Benchmark* benchmark) {
    benchmark->ArgName("nodes");
    for (const int nodeCount : kScaleNodeCounts) benchmark->Arg(nodeCount);
}

void addRepresentativeCases(benchmark::Benchmark* benchmark) {
    benchmark->ArgName("nodes");
    for (const int nodeCount : kRepresentativeNodeCounts) benchmark->Arg(nodeCount);
}

void addStateCases(benchmark::Benchmark* benchmark) {
    benchmark->ArgName("nodes");
    for (const int nodeCount : kStateNodeCounts) benchmark->Arg(nodeCount);
}

void addCacheCases(benchmark::Benchmark* benchmark) {
    benchmark->ArgName("nodes");
    for (const int nodeCount : kCacheNodeCounts) benchmark->Arg(nodeCount);
}

void addDirectionCases(benchmark::Benchmark* benchmark) {
    benchmark->ArgName("nodes");
    for (const int nodeCount : kDirectionNodeCounts) benchmark->Arg(nodeCount);
}

BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, FlatColumn, LayoutCase::FlatColumn)->Apply(addScaleCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, FlatRow, LayoutCase::FlatRow)->Apply(addScaleCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, BalancedTree, LayoutCase::BalancedTree)->Apply(addScaleCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, FlexRow, LayoutCase::FlexRow)->Apply(addScaleCases);

BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, Normal, LayoutCase::Normal)->Apply(addRepresentativeCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, ShortLabels, LayoutCase::ShortLabels)->Apply(addRepresentativeCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, WrappedText, LayoutCase::WrappedText)->Apply(addRepresentativeCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, CompositeControls, LayoutCase::CompositeControls)->Apply(addRepresentativeCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, HiddenLabels, LayoutCase::HiddenLabels)->Apply(addStateCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, CollapsedLabels, LayoutCase::CollapsedLabels)->Apply(addStateCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, RightToLeftRow, LayoutCase::RightToLeftRow)->Apply(addStateCases);

BENCHMARK_CAPTURE(BM_Layout_CachedLayout, FlatColumn, LayoutCase::FlatColumn)->Apply(addCacheCases);
BENCHMARK_CAPTURE(BM_Layout_CachedLayout, WrappedText, LayoutCase::WrappedText)->Apply(addCacheCases);
BENCHMARK_CAPTURE(BM_Layout_CachedLayout, CompositeControls, LayoutCase::CompositeControls)->Apply(addCacheCases);
BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterDirectionChange, FlatRow, LayoutCase::FlatRow)->Apply(addDirectionCases);
} // namespace
