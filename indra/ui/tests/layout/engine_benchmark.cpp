/**
 * @file engine_benchmark.cpp
 * @brief Benchmarks retained Radia UI layout transactions.
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
 * Foundation, Inc., 51 Battery Street, San Francisco, CA 02110-1301 USA
 * $/LicenseInfo$
 */

#include "linden_common.h"
#include <algorithm>
#include <array>
#include <benchmark/benchmark.h>
#include <cstddef>
#include <cstdlib>
#include <memory>
#include <string>
#include "layout/engine.h"
#include "style/stylesheet.h"
#include "text/metrics.h"
#include "widgets/button.h"
#include "widgets/label.h"
#include "widgets/panel.h"
#include "widgets/switch.h"
#include "widgets/text.h"

namespace {
using radia::ui::Button;
using radia::ui::fixedTextMetrics;
using radia::ui::Label;
using radia::ui::LayoutDirection;
using radia::ui::LayoutStatistics;
using radia::ui::layoutTree;
using radia::ui::Panel;
using radia::ui::Rect;
using radia::ui::StyleSheet;
using radia::ui::Switch;
using radia::ui::Text;
using radia::ui::Visibility;

enum class LayoutCase {
    FlatColumn,
    FlatRow,
    BalancedTree,
    FlexRow,
    FreeFlow,
    ShortLabels,
    WrappedText,
    CompositeControls,
    HiddenLabels,
    CollapsedLabels,
    RightToLeftRow
};

struct LayoutFixture {
    std::unique_ptr<Panel> root;
    StyleSheet styleSheet;
    LayoutDirection direction = LayoutDirection::LeftToRight;
};

constexpr std::array<int, 5> kScaleNodeCounts = {0, 1, 10, 100, 1000};
constexpr std::array<int, 2> kRepresentativeNodeCounts = {100, 1000};
constexpr std::array<int, 1> kStateNodeCounts = {1000};
constexpr std::array<int, 1> kCacheNodeCounts = {1000};
constexpr std::array<int, 2> kDirectionNodeCounts = {100, 1000};

void addFlatLabels(Panel& root, std::size_t nodeCount, bool withText, Visibility specialVisibility = Visibility::Visible) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        auto label = std::make_unique<Label>(withText ? "Item " + std::to_string(index) : std::string());
        if (specialVisibility != Visibility::Visible && index % 4 == 0) label->setVisibility(specialVisibility);
        root.addChild(std::move(label));
    }
}

void addBalancedChildren(Panel& parent, std::size_t& remaining, std::size_t depth) {
    if (remaining == 0) return;

    const std::size_t branchCount = depth % 2 == 0 ? 2 : 3;
    for (std::size_t branch = 0; branch < branchCount && remaining > 0; ++branch) {
        if (remaining > 1 && depth < 8) {
            auto child = std::make_unique<Panel>();
            Panel* childPointer = child.get();
            parent.addChild(std::move(child));
            --remaining;
            addBalancedChildren(*childPointer, remaining, depth + 1);
        } else {
            parent.addChild(std::make_unique<Label>());
            --remaining;
        }
    }
}

void addFreeFlowLabels(Panel& root, std::size_t nodeCount) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        auto label = std::make_unique<Label>();
        const float x = static_cast<float>(index % 32) * 18.f;
        const float y = static_cast<float>(index / 32) * 14.f;
        label->setRect({x, y, 14.f, 10.f});
        root.addChild(std::move(label));
    }
}

void addCompositeControls(Panel& root, std::size_t nodeCount) {
    for (std::size_t index = 0; index < nodeCount; ++index) {
        switch (index % 3) {
            case 0: {
                auto button = std::make_unique<Button>();
                button->setIcon("search");
                button->setLabel("Apply");
                root.addChild(std::move(button));
                break;
            }
            case 1: {
                auto control = std::make_unique<Switch>();
                control->setChecked(index % 2 == 0);
                root.addChild(std::move(control));
                break;
            }
            default: root.addChild(std::make_unique<Label>("Status")); break;
        }
    }
}

LayoutFixture makeFixture(LayoutCase layoutCase, std::size_t nodeCount) {
    LayoutFixture fixture;
    fixture.root = std::make_unique<Panel>();

    std::string styleSource;
    Rect rootRect;
    switch (layoutCase) {
        case LayoutCase::FlatColumn:
            styleSource = "panel { flow: column; gap: 2px; } label { width: 120px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        case LayoutCase::FlatRow:
            styleSource = "panel { flow: row; gap: 1px; } label { width: 10px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f), 24.f};
            break;
        case LayoutCase::BalancedTree: {
            styleSource = "panel { flow: row; gap: 1px; } label { width: 10px; height: 10px; }";
            std::size_t remaining = nodeCount;
            addBalancedChildren(*fixture.root, remaining, 0);
            rootRect = {0.f, 0.f, 800.f, 600.f};
            break;
        }
        case LayoutCase::FlexRow:
            styleSource = "panel { flow: row; gap: 2px; } label { flex: 1; min-width: 0px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            rootRect = {0.f, 0.f, 1200.f, 24.f};
            break;
        case LayoutCase::FreeFlow:
            styleSource = "panel { flow: free; } label { width: 14px; height: 10px; }";
            addFreeFlowLabels(*fixture.root, nodeCount);
            rootRect = {0.f, 0.f, 600.f, std::max(200.f, static_cast<float>((nodeCount + 31) / 32) * 14.f + 20.f)};
            break;
        case LayoutCase::ShortLabels:
            styleSource = "panel { flow: column; gap: 2px; } label { width: 180px; height: 18px; font-size: 12px; line-height: 18px; }";
            addFlatLabels(*fixture.root, nodeCount, true);
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 20.f)};
            break;
        case LayoutCase::WrappedText:
            styleSource = "panel { flow: column; gap: 2px; } text { width: 48px; font-size: 10px; line-height: 10px; text-wrap: wrap; }";
            for (std::size_t index = 0; index < nodeCount; ++index) fixture.root->addChild(std::make_unique<Text>("alpha beta gamma delta"));
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 42.f)};
            break;
        case LayoutCase::CompositeControls:
            styleSource = "panel { flow: column; gap: 2px; } "
                          "button { width: 160px; height: 24px; padding: 4px; gap: 4px; flow: row; } "
                          "button > icon { size: 16px; } switch { width: 64px; height: 24px; } "
                          "switch::thumb { size: 18px; } label { width: 160px; height: 18px; }";
            addCompositeControls(*fixture.root, nodeCount);
            rootRect = {0.f, 0.f, 240.f, std::max(120.f, static_cast<float>(nodeCount) * 28.f)};
            break;
        case LayoutCase::HiddenLabels:
            styleSource = "panel { flow: column; gap: 2px; } label { width: 120px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false, Visibility::Hidden);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        case LayoutCase::CollapsedLabels:
            styleSource = "panel { flow: column; gap: 2px; } label { width: 120px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false, Visibility::Collapsed);
            rootRect = {0.f, 0.f, 320.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f)};
            break;
        case LayoutCase::RightToLeftRow:
            styleSource = "panel { flow: row; gap: 1px; } label { width: 10px; height: 10px; }";
            addFlatLabels(*fixture.root, nodeCount, false);
            fixture.direction = LayoutDirection::RightToLeft;
            rootRect = {0.f, 0.f, std::max(120.f, static_cast<float>(nodeCount) * 12.f), 24.f};
            break;
    }

    fixture.root->setRect(rootRect);
    if (!fixture.styleSheet.loadRadia(styleSource, "engine_benchmark.radia").ok()) std::abort();
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

BENCHMARK_CAPTURE(BM_Layout_RelayoutAfterResize, FreeFlow, LayoutCase::FreeFlow)->Apply(addRepresentativeCases);
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
