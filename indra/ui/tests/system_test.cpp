/**
 * @file system_test.cpp
 * @brief Tests System generations, locale changes, and Surface notifications.
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
#include <gtest/gtest.h>
#include <memory>
#include <string>
#include <utility>
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/label.h"
#include "widgets/text.h"

namespace {
using radia::ui::fixedTextMetrics;
using radia::ui::KeybindingPresentation;
using radia::ui::Label;
using radia::ui::LayoutBuildResult;
using radia::ui::PublicationCommit;
using radia::ui::ResourceSnapshot;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::Text;
using radia::ui::Widget;

constexpr char kEmptyLocalization[] = "defaultLocale: en\nlocales: {en: {name: English, strings: {}}}\n";

std::string singleStringLocalization(const std::string& key, const std::string& value) {
    return "defaultLocale: en\nlocales: {en: {name: English, strings: {" + key + ": \"" + value + "\"}}}\n";
}

ResourceSnapshot skinSnapshot(std::string localization = kEmptyLocalization, std::string style = {}) {
    if (localization.empty()) localization = kEmptyLocalization;
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", std::move(localization));
    snapshot.add("skin.radia", std::move(style));
    return snapshot;
}

float resolvedLabelWidth(const System& system) {
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(200.f, 100.f);
    auto label = std::make_unique<Label>();
    Label* labelPtr = label.get();
    surface->mount(std::move(label));
    surface->updateLayout();
    return labelPtr->rect().w;
}
} // namespace

class LocaleProbe final : public Widget {
public:
    LocaleProbe() : Widget("locale-probe") {}
    int notifications() const { return mNotifications; }

private:
    void onLocaleChanged(const System&) override { ++mNotifications; }
    int mNotifications = 0;
};

TEST(SystemTest, PublishesLocalizationStylesIconsAndWidgetResources) {
    constexpr char kPublishedStyles[] = "label { width: 40px; }";
    constexpr char kViewMarkup[] = "<p id=\"message\">message</p>";
    constexpr char kSearchIcon[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>";

    ResourceSnapshot snapshot = skinSnapshot(singleStringLocalization("message", "Ready"), kPublishedStyles);
    snapshot.add("view.xml", kViewMarkup);
    snapshot.add("resources/icons/search.svg", kSearchIcon);

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(system.resolveText("message"), "Ready");
    EXPECT_EQ(resolvedLabelWidth(system), 40.f);
    EXPECT_TRUE(system.hasIcon("search"));

    const LayoutBuildResult buildResult = system.buildWidgetTree("view.xml");
    ASSERT_TRUE(buildResult.ok());
    const Text* text = buildResult.rootAs<Text>();
    ASSERT_NE(text, nullptr);
    EXPECT_EQ(text->text(), "Ready");

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(100.f, 100.f);
    auto styled = std::make_unique<Label>();
    Label* styledPtr = styled.get();
    surface->mount(std::move(styled));
    surface->updateLayout();
    EXPECT_EQ(styledPtr->rect().w, 40.f);
}

TEST(SystemTest, RejectsInvalidCandidatesWithoutReplacingLiveGeneration) {
    constexpr char kLiveStyles[] = "label { width: 40px; }";
    constexpr char kInvalidLocalization[] = "defaultLocale: [";
    constexpr char kInvalidStyles[] = "label { display: sideways; width: 90px; }";

    System system;
    const SkinGenerationPrepareResult live = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Live"), kLiveStyles));
    ASSERT_TRUE(live.ok());
    ASSERT_TRUE(system.publish(live.generation));

    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(skinSnapshot(kInvalidLocalization, kInvalidStyles));
    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(system.resolveText("message"), "Live");
    EXPECT_EQ(resolvedLabelWidth(system), 40.f);
}

TEST(SystemTest, RejectsEmptyReferencedIcons) {
    constexpr char kIconStyles[] = "icon { size: 16px; }";

    ResourceSnapshot snapshot = skinSnapshot({}, kIconStyles);
    snapshot.add("resources/icons/search.svg", "");
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
}

TEST(SystemTest, RejectsUnknownIconsInLayoutResources) {
    constexpr char kIconStyles[] = "icon { size: 16px; }";
    constexpr char kKnownIconMarkup[] = "<icon src=\"actions/search\"/>";
    constexpr char kMissingIconMarkup[] = "<icon src=\"actions/missing\"/>";
    constexpr char kSearchIcon[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>";

    ResourceSnapshot snapshot = skinSnapshot({}, kIconStyles);
    snapshot.add("known.xml", kKnownIconMarkup);
    snapshot.add("missing.xml", kMissingIconMarkup);
    snapshot.add("resources/icons/actions/search.svg", kSearchIcon);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
}

TEST(SystemTest, RejectsSnapshotsWithoutLocalization) {
    constexpr char kStylesWithoutLocalization[] = "label { width: 40px; }";

    ResourceSnapshot snapshot;
    snapshot.add("skin.radia", kStylesWithoutLocalization);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().source, "localization.yaml");
}

TEST(SystemTest, RejectsMalformedIcons) {
    constexpr char kIconStyles[] = "icon { size: 16px; }";
    constexpr char kMalformedIcon[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10\"/></svg>";

    ResourceSnapshot snapshot = skinSnapshot({}, kIconStyles);
    snapshot.add("resources/icons/search.svg", kMalformedIcon);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().source, "resources/icons/search.svg");
}

TEST(SystemTest, UpdatesMountedWidgetsWhenLocaleChanges) {
    constexpr char kMultilingualLocalization[] = "defaultLocale: en\nlocales: {en: {name: English, strings: {message: Ready}}, "
                                                 "pt: {name: Português, strings: {message: Pronto}}, "
                                                 "ar: {name: العربية, direction: rtl, strings: {message: جاهز}}}\n";

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot(kMultilingualLocalization));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);
    auto localized = std::make_unique<Label>();
    Label* localizedPtr = localized.get();
    localized->setContent(system.localize("message"));
    surface->root().addChild(std::move(localized));
    auto probe = std::make_unique<LocaleProbe>();
    LocaleProbe* probePtr = probe.get();
    surface->root().addChild(std::move(probe));

    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(localizedPtr->text(), "Pronto");
    EXPECT_EQ(probePtr->notifications(), 2);
}

TEST(SystemTest, PreservesLocaleAcrossPublicationAndFallsBackWhenRemoved) {
    constexpr char kMultilingualLocalization[] = "defaultLocale: en\nlocales: {en: {name: English, strings: {message: Ready}}, "
                                                 "pt: {name: Português, strings: {message: Pronto}}, "
                                                 "ar: {name: العربية, direction: rtl, strings: {message: جاهز}}}\n";

    System system;
    const SkinGenerationPrepareResult initial = SkinCompiler().prepare(skinSnapshot(kMultilingualLocalization));
    ASSERT_TRUE(initial.ok());
    ASSERT_TRUE(system.publish(initial.generation));
    ASSERT_TRUE(system.setLocale("pt"));

    const SkinGenerationPrepareResult compatible = SkinCompiler().prepare(skinSnapshot(kMultilingualLocalization));
    ASSERT_TRUE(compatible.ok());
    ASSERT_TRUE(system.publish(compatible.generation));
    EXPECT_EQ(system.activeLocale(), "pt");

    const SkinGenerationPrepareResult fallback = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Ready again")));
    ASSERT_TRUE(fallback.ok());
    ASSERT_TRUE(system.publish(fallback.generation));
    EXPECT_EQ(system.activeLocale(), "en");
    EXPECT_EQ(system.resolveText("message"), "Ready again");
}

TEST(SystemTest, PublishesGenerationUpdatesToExistingSurfacesAndNewTrees) {
    constexpr char kLiveStyles[] = "label { width: 40px; }";
    constexpr char kCandidateStyles[] = "label { width: 90px; }";
    constexpr char kViewMarkup[] = "<p>message</p>";

    System system;
    const SkinGenerationPrepareResult live = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Old"), kLiveStyles));
    ASSERT_TRUE(live.ok());
    ASSERT_TRUE(system.publish(live.generation));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);
    auto styled = std::make_unique<Label>();
    Label* styledPtr = styled.get();
    surface->mount(std::move(styled));
    surface->updateLayout();
    EXPECT_EQ(styledPtr->rect().w, 40.f);

    ResourceSnapshot snapshot = skinSnapshot(singleStringLocalization("message", "New"), kCandidateStyles);
    snapshot.add("view.xml", kViewMarkup);
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(system.resolveText("message"), "Old");

    const LayoutBuildResult candidateBuild = prepared.generation->buildWidgetTree("view.xml", system.activeLocale());
    ASSERT_TRUE(candidateBuild.ok());
    const Text* candidateText = candidateBuild.rootAs<Text>();
    ASSERT_NE(candidateText, nullptr);
    EXPECT_EQ(candidateText->text(), "New");

    ASSERT_TRUE(system.publish(prepared.generation));
    EXPECT_EQ(system.generation(), 2ULL);
    EXPECT_EQ(system.resolveText("message"), "New");
    surface->updateLayout();
    EXPECT_EQ(styledPtr->rect().w, 90.f);

    const LayoutBuildResult liveBuild = system.buildWidgetTree("view.xml");
    ASSERT_TRUE(liveBuild.ok());
    const Text* liveText = liveBuild.rootAs<Text>();
    ASSERT_NE(liveText, nullptr);
    EXPECT_EQ(liveText->text(), "New");
}

TEST(SystemTest, RejectsInvalidUnmountedLayoutResources) {
    constexpr char kStyles[] = "label { width: 90px; }";
    constexpr char kValidMarkup[] = "<p>Ready</p>";
    constexpr char kUnsupportedMarkup[] = "<unsupported/>";

    ResourceSnapshot snapshot = skinSnapshot({}, kStyles);
    snapshot.add("valid.xml", kValidMarkup);
    snapshot.add("unused.xml", kUnsupportedMarkup);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
}

TEST(SystemTest, RefreshesKbdPresentationWhenKeybindingsChange) {
    constexpr char kKeybindingLocalization[] = "defaultLocale: en\nlocales: {en: {name: English, strings: "
                                               "{fly.label: 'Fly <kbd shortcut=\"toggle-fly\"/>'}}}\n";
    constexpr char kViewMarkup[] = "<p>fly.label</p>";

    ResourceSnapshot snapshot = skinSnapshot(kKeybindingLocalization);
    snapshot.add("view.xml", kViewMarkup);
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    KeybindingPresentation presentation{{"F"}};
    System system;
    system.setKeybindingResolver(
        [&presentation](const std::string& binding) { return binding == "toggle-fly" ? presentation : KeybindingPresentation{}; });
    ASSERT_TRUE(system.publish(prepared.generation));

    LayoutBuildResult buildResult = system.buildWidgetTree("view.xml");
    ASSERT_TRUE(buildResult.ok());
    const Text* text = buildResult.rootAs<Text>();
    ASSERT_NE(text, nullptr);

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);
    surface->mount(std::move(buildResult.root));
    surface->updateLayout();
    EXPECT_EQ(text->text(), "Fly F");
    const float initialWidth = text->desiredSize().x;

    presentation = {{"Ctrl", "F"}};
    system.refreshKeybindings();
    surface->updateLayout();
    EXPECT_EQ(text->text(), "Fly Ctrl F");
    EXPECT_GT(text->desiredSize().x, initialWidth);
}

TEST(SystemTest, RollsBackPublicationWhenCommitRejectsCandidate) {
    System system;
    const SkinGenerationPrepareResult live = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Old")));
    ASSERT_TRUE(live.ok());
    ASSERT_TRUE(system.publish(live.generation));

    const SkinGenerationPrepareResult candidate = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "New")));
    ASSERT_TRUE(candidate.ok());

    bool callbackSawCandidate = false;
    class RejectingCommit final : public PublicationCommit {
    public:
        RejectingCommit(System& system, bool& sawCandidate) : mSystem(system), mSawCandidate(sawCandidate) {}

        bool commit() override {
            mSawCandidate = mSystem.resolveText("message") == "New";
            return false;
        }

    private:
        System& mSystem;
        bool& mSawCandidate;
    } rejectingCommit(system, callbackSawCandidate);

    EXPECT_FALSE(system.publish(candidate.generation, rejectingCommit));
    EXPECT_TRUE(callbackSawCandidate);
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(system.resolveText("message"), "Old");
}
