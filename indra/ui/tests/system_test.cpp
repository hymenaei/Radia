/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include <cstdint>
#include <functional>
#include <gtest/gtest.h>
#include <memory>
#include <string>
#include <utility>
#include "dom/elementinternal.h"
#include "dom/text.h"
#include "html/button.h"
#include "html/element.h"
#include "html/input.h"
#include "html/label.h"
#include "html/panel.h"
#include "nativeappearance.h"
#include "render/recordingpaintcontext.h"
#include "resourceprovider.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace {
using radia::ui::AppearanceMode;
using radia::ui::Element;
using radia::ui::fixedTextMetrics;
using radia::ui::HTMLButtonElement;
using radia::ui::HTMLElement;
using radia::ui::HTMLInputElement;
using radia::ui::HTMLLabelElement;
using radia::ui::HTMLPanelElement;
using radia::ui::KeybindingPresentation;
using radia::ui::NativeAppearance;
using radia::ui::NativeAppearanceBase;
using radia::ui::NativeButtonPaintRequest;
using radia::ui::NativeScrollbarMetrics;
using radia::ui::NativeScrollbarPaintRequest;
using radia::ui::NodeList;
using radia::ui::PaintCommand;
using radia::ui::PaintCommandKind;
using radia::ui::PublicationCommit;
using radia::ui::RecordingPaintContext;
using radia::ui::ResourceBuildResult;
using radia::ui::ResourceId;
using radia::ui::ResourceSnapshot;
using radia::ui::ScrollbarAxis;
using radia::ui::ScrollbarMode;
using radia::ui::ScrollLayoutOptions;
using radia::ui::SkinCompiler;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::Style;
using radia::ui::Surface;
using radia::ui::System;
using radia::ui::detail::makeElement;
using radia::ui::detail::makeElementValue;

constexpr char kEmptyLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {}}}\n";
constexpr NativeScrollbarMetrics kExpectedClassicScrollbarMetrics{15.f, 20.f, 15.f, 3.f};
constexpr float kExpectedScrollbarTrackRed = .08f;
constexpr float kExpectedScrollbarTrackGreen = .09f;
constexpr float kExpectedScrollbarTrackBlue = .1f;
constexpr float kExpectedScrollbarTrackAlpha = .52f;
constexpr float kExpectedThumbCornerRadius = 4.f;
constexpr float kMinimumHoveredThumbRed = .55f;

std::string singleStringLocalization(const std::string& key, const std::string& value) {
    return "defaultLocale: en\nlocales: {en: {strings: {" + key + ": \"" + value + "\"}}}\n";
}

ResourceSnapshot skinSnapshot(std::string localization = kEmptyLocalization, std::string style = {}) {
    if (localization.empty()) localization = kEmptyLocalization;
    ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", std::move(localization));
    snapshot.add("skin.css", std::move(style));
    return snapshot;
}

float resolvedLabelWidth(const System& system) {
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    surface->setViewport(200.f, 100.f);
    auto label = makeElement<HTMLLabelElement>();
    HTMLLabelElement* labelPtr = label.get();
    surface->mount(std::move(label));
    surface->updateLayout();
    return labelPtr->rect().w;
}
} // namespace

TEST(NativeAppearanceTest, BaseOwnsScrollbarDefaultsAndStateStyling) {
    NativeAppearanceBase appearance;
    const NativeScrollbarMetrics metrics = appearance.scrollbarMetrics(ScrollbarMode::Classic);
    EXPECT_FLOAT_EQ(metrics.thickness, kExpectedClassicScrollbarMetrics.thickness);
    EXPECT_FLOAT_EQ(metrics.minimumThumbLength, kExpectedClassicScrollbarMetrics.minimumThumbLength);
    EXPECT_FLOAT_EQ(metrics.arrowLength, kExpectedClassicScrollbarMetrics.arrowLength);
    EXPECT_FLOAT_EQ(metrics.thumbPadding, kExpectedClassicScrollbarMetrics.thumbPadding);

    NativeScrollbarPaintRequest request;
    request.geometry.vertical.thumb = {0.f, 0.f, 8.f, 20.f};
    request.vertical.hoveredPart = radia::ui::ScrollbarPart::Thumb;
    const auto style = appearance.scrollbarPaintStyle(request, ScrollbarAxis::Vertical);
    EXPECT_FLOAT_EQ(style.track.r, kExpectedScrollbarTrackRed);
    EXPECT_FLOAT_EQ(style.track.g, kExpectedScrollbarTrackGreen);
    EXPECT_FLOAT_EQ(style.track.b, kExpectedScrollbarTrackBlue);
    EXPECT_FLOAT_EQ(style.track.a, kExpectedScrollbarTrackAlpha);
    EXPECT_FLOAT_EQ(style.thumbRadius, kExpectedThumbCornerRadius);
    EXPECT_GT(style.thumb.r, kMinimumHoveredThumbRed);
}

TEST(NativeAppearanceTest, ButtonPaintDispatchUsesNativeAppearanceForAutoMode) {
    auto button = makeElementValue<HTMLButtonElement>();
    RecordingPaintContext recording;
    Style style;
    style.appearance = AppearanceMode::Auto;

    button.paint(recording, style, 1.25f);

    const PaintCommand* command = recording.last(PaintCommandKind::NativeButton);
    ASSERT_NE(command, nullptr);
    ASSERT_TRUE(command->nativeButton.has_value());
    EXPECT_FLOAT_EQ(command->nativeButton->scale, 1.25f);
    EXPECT_FLOAT_EQ(command->nativeButton->bounds.x, button.rect().x);
    EXPECT_FLOAT_EQ(command->nativeButton->bounds.y, button.rect().y);
    EXPECT_FLOAT_EQ(command->nativeButton->bounds.w, button.rect().w);
    EXPECT_FLOAT_EQ(command->nativeButton->bounds.h, button.rect().h);
}

TEST(NativeAppearanceTest, ButtonPaintDispatchKeepsUnstyledModeOnCssPath) {
    auto button = makeElementValue<HTMLButtonElement>();
    RecordingPaintContext recording;
    Style style;
    style.appearance = AppearanceMode::Unstyled;

    button.paint(recording, style, 1.f);

    EXPECT_EQ(recording.count(PaintCommandKind::NativeButton), 0U);
    EXPECT_EQ(recording.count(PaintCommandKind::Box), 1U);
}

class LocaleProbe final : public Element {
public:
    LocaleProbe() : Element("locale-probe") {}
    int notifications() const { return mNotifications; }

private:
    void onLocaleChanged(const System&) override { ++mNotifications; }
    int mNotifications = 0;
};

struct NotificationCounts {
    int locale = 0;
    int keybindings = 0;
    bool armed = false;
};

class SurfaceNotificationProbe final : public HTMLElement {
public:
    using Callback = std::function<void()>;

    SurfaceNotificationProbe(std::shared_ptr<NotificationCounts> counts, Callback onLocaleChanged = {}, Callback onKeybindingsChanged = {})
        : HTMLElement("surface-notification-probe"), mCounts(std::move(counts)), mOnLocaleChanged(std::move(onLocaleChanged)),
          mOnKeybindingsChanged(std::move(onKeybindingsChanged)) {}

private:
    void onLocaleChanged(const System&) override {
        if (!mCounts->armed) return;
        ++mCounts->locale;
        if (mOnLocaleChanged) mOnLocaleChanged();
    }

    void onKeybindingsChanged(const System&) override {
        if (!mCounts->armed) return;
        ++mCounts->keybindings;
        if (mOnKeybindingsChanged) mOnKeybindingsChanged();
    }

    std::shared_ptr<NotificationCounts> mCounts;
    Callback mOnLocaleChanged;
    Callback mOnKeybindingsChanged;
};

class TestNativeAppearance final : public NativeAppearanceBase {
public:
    TestNativeAppearance(NativeScrollbarMetrics metrics, std::uint64_t revision) : TestNativeAppearance(metrics, metrics, revision) {}
    TestNativeAppearance(NativeScrollbarMetrics classicMetrics, NativeScrollbarMetrics overlayMetrics, std::uint64_t revision)
        : mClassicMetrics(classicMetrics), mOverlayMetrics(overlayMetrics), mRevision(revision) {}

    NativeScrollbarMetrics scrollbarMetrics(ScrollbarMode mode) const override {
        return mode == ScrollbarMode::Classic ? mClassicMetrics : mOverlayMetrics;
    }
    std::uint64_t revision() const noexcept override { return mRevision; }

private:
    NativeScrollbarMetrics mClassicMetrics;
    NativeScrollbarMetrics mOverlayMetrics;
    std::uint64_t mRevision;
};

class ReentrantNativeAppearance final : public NativeAppearanceBase {
public:
    explicit ReentrantNativeAppearance(std::function<void()> callback) : mCallback(std::move(callback)) {}

    std::uint64_t revision() const noexcept override {
        if (!mCallbackInvoked) {
            mCallbackInvoked = true;
            mCallback();
        }
        return 2;
    }

private:
    mutable std::function<void()> mCallback;
    mutable bool mCallbackInvoked = false;
};

TEST(SystemTest, PublishesLocalizationStylesIconsAndElementResources) {
    constexpr char kPublishedStyles[] = "label { width: 40px; }";
    constexpr char kViewHTML[] = "<p id=\"message\">{{message}}</p>";
    constexpr char kSearchIcon[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>";

    ResourceSnapshot snapshot = skinSnapshot(singleStringLocalization("message", "Ready"), kPublishedStyles);
    snapshot.add("view.html", kViewHTML);
    snapshot.add("resources/icons/search.svg", kSearchIcon);

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(system.resolveText("message"), "Ready");
    EXPECT_FLOAT_EQ(resolvedLabelWidth(system), 40.f);
    EXPECT_TRUE(system.hasIcon("search"));

    const ResourceBuildResult buildResult = system.buildElementTree(ResourceId("view.html"));
    ASSERT_TRUE(buildResult.ok());
    const Element* text = buildResult.rootAs<Element>();
    ASSERT_NE(text, nullptr);
    EXPECT_EQ(text->textContent(), "Ready");

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(100.f, 100.f);
    auto styled = makeElement<HTMLLabelElement>();
    HTMLLabelElement* styledPtr = styled.get();
    surface->mount(std::move(styled));
    surface->updateLayout();
    EXPECT_FLOAT_EQ(styledPtr->rect().w, 40.f);
}

TEST(SystemTest, NativeAppearanceMetricsDriveLayoutAndPaintRevision) {
    constexpr char kScrollStyles[] = "#viewport { display: block; overflow: scroll; scrollbar-mode: classic; }";
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot({}, kScrollStyles));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    system.setNativeAppearance(std::make_shared<TestNativeAppearance>(NativeScrollbarMetrics{20.f, 24.f, 20.f, 3.f}, 42));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 200.f);
    auto viewport = makeElement<HTMLPanelElement>();
    viewport->setId("viewport").setRect({0.f, 0.f, 100.f, 100.f});
    auto content = makeElement<HTMLPanelElement>();
    content->setRect({0.f, 0.f, 180.f, 180.f});
    viewport->append(std::move(content));
    radia::ui::HTMLPanelElement* viewportPtr = viewport.get();
    surface->mount(std::move(viewport));
    surface->updateLayout();
    EXPECT_FLOAT_EQ(viewportPtr->clientWidth(), 80.f);
    EXPECT_FLOAT_EQ(viewportPtr->clientHeight(), 80.f);

    RecordingPaintContext initialPaint;
    surface->paint(initialPaint);
    const PaintCommand* initialScrollbar = initialPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(initialScrollbar, nullptr);
    ASSERT_TRUE(initialScrollbar->scrollbar.has_value());
    EXPECT_FLOAT_EQ(initialScrollbar->scrollbar->metrics.thickness, 20.f);
    EXPECT_EQ(initialScrollbar->scrollbar->appearanceRevision, 42u);

    system.setNativeAppearance(std::make_shared<TestNativeAppearance>(NativeScrollbarMetrics{24.f, 28.f, 24.f, 3.f}, 43));
    EXPECT_TRUE(surface->needsPaint());
    surface->updateLayout();
    EXPECT_FLOAT_EQ(viewportPtr->clientWidth(), 76.f);
    EXPECT_FLOAT_EQ(viewportPtr->clientHeight(), 76.f);

    RecordingPaintContext updatedPaint;
    surface->paint(updatedPaint);
    const PaintCommand* updatedScrollbar = updatedPaint.last(PaintCommandKind::Scrollbar);
    ASSERT_NE(updatedScrollbar, nullptr);
    ASSERT_TRUE(updatedScrollbar->scrollbar.has_value());
    EXPECT_FLOAT_EQ(updatedScrollbar->scrollbar->metrics.thickness, 24.f);
    EXPECT_EQ(updatedScrollbar->scrollbar->appearanceRevision, 43u);
}

TEST(SystemTest, NativeAppearanceMetricsFollowAuthoredScrollbarMode) {
    constexpr char kScrollStyles[] = "#classic { display: block; overflow: scroll; scrollbar-mode: classic; } "
                                     "#overlay { display: block; overflow: scroll; }";
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot({}, kScrollStyles));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    system.setNativeAppearance(
        std::make_shared<TestNativeAppearance>(NativeScrollbarMetrics{20.f, 24.f, 20.f, 3.f}, NativeScrollbarMetrics{8.f, 10.f, 8.f, 2.f}, 44));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(240.f, 120.f);
    ScrollLayoutOptions options;
    options.scrollbarMode = ScrollbarMode::Overlay;
    surface->setScrollLayoutOptions(options);

    auto classic = makeElement<HTMLPanelElement>();
    classic->setId("classic").setRect({0.f, 0.f, 100.f, 100.f});
    auto classicContent = makeElement<HTMLPanelElement>();
    classicContent->setRect({0.f, 0.f, 180.f, 180.f});
    classic->append(std::move(classicContent));
    radia::ui::HTMLPanelElement* classicPtr = classic.get();

    auto overlay = makeElement<HTMLPanelElement>();
    overlay->setId("overlay").setRect({120.f, 0.f, 100.f, 100.f});
    auto overlayContent = makeElement<HTMLPanelElement>();
    overlayContent->setRect({0.f, 0.f, 180.f, 180.f});
    overlay->append(std::move(overlayContent));
    radia::ui::HTMLPanelElement* overlayPtr = overlay.get();

    surface->mount(std::move(classic));
    surface->mount(std::move(overlay));
    surface->updateLayout();

    EXPECT_FLOAT_EQ(classicPtr->clientWidth(), 80.f);
    EXPECT_FLOAT_EQ(classicPtr->clientHeight(), 80.f);
    EXPECT_FLOAT_EQ(overlayPtr->clientWidth(), 100.f);
    EXPECT_FLOAT_EQ(overlayPtr->clientHeight(), 100.f);

    RecordingPaintContext recording;
    surface->paint(recording);
    const PaintCommand* classicScrollbar = nullptr;
    const PaintCommand* overlayScrollbar = nullptr;
    for (const PaintCommand& command : recording.commands()) {
        if (command.kind != PaintCommandKind::Scrollbar || !command.scrollbar) continue;
        if (command.scrollbar->mode == ScrollbarMode::Classic) classicScrollbar = &command;
        else overlayScrollbar = &command;
    }
    ASSERT_NE(classicScrollbar, nullptr);
    ASSERT_NE(overlayScrollbar, nullptr);
    EXPECT_FLOAT_EQ(classicScrollbar->scrollbar->metrics.thickness, 20.f);
    EXPECT_FLOAT_EQ(overlayScrollbar->scrollbar->metrics.thickness, 8.f);
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
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "localization.yaml.invalid");
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
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "svg.empty");
}

TEST(SystemTest, RejectsUnknownIconsInLayoutResources) {
    constexpr char kIconStyles[] = "icon { size: 16px; }";
    constexpr char kKnownIconHTML[] = "<icon src=\"actions/search\"></icon>";
    constexpr char kMissingIconHTML[] = "<icon src=\"actions/missing\"></icon>";
    constexpr char kSearchIcon[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>";

    ResourceSnapshot snapshot = skinSnapshot({}, kIconStyles);
    snapshot.add("known.html", kKnownIconHTML);
    snapshot.add("missing.html", kMissingIconHTML);
    snapshot.add("resources/icons/actions/search.svg", kSearchIcon);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "layout.icon.missing");
}

TEST(SystemTest, RejectsSnapshotsWithoutLocalization) {
    constexpr char kStylesWithoutLocalization[] = "label { width: 40px; }";

    ResourceSnapshot snapshot;
    snapshot.add("skin.css", kStylesWithoutLocalization);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "ui.resource.missing");
    EXPECT_EQ(rejected.errors.front().source, "localization.yaml");
}

TEST(SystemTest, RejectsMalformedIcons) {
    constexpr char kIconStyles[] = "icon { size: 16px; }";
    constexpr char kMalformedIcon[] = "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10\"/></svg>";

    ResourceSnapshot snapshot = skinSnapshot({}, kIconStyles);
    snapshot.add("resources/icons/search.svg", kMalformedIcon, "skin/views/resources/icons/search.svg");
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "svg.path.arguments_invalid");
    EXPECT_EQ(rejected.errors.front().source, "skin/views/resources/icons/search.svg");
}

TEST(SystemTest, PreservesPhysicalProvenanceForUnsupportedAssets) {
    ResourceSnapshot snapshot = skinSnapshot();
    snapshot.add("resources/not-an-icon.txt", "not an icon", "skin/views/resources/not-an-icon.txt");

    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "ui.asset.unsupported");
    EXPECT_EQ(rejected.errors.front().source, "skin/views/resources/not-an-icon.txt");
}

TEST(SystemTest, PreservesPhysicalProvenanceForUnsupportedLayouts) {
    ResourceSnapshot snapshot = skinSnapshot();
    snapshot.add("broken.txt", "<panel></panel>", "skin/ui/broken.txt");

    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "ui.layout.unsupported");
    EXPECT_EQ(rejected.errors.front().source, "skin/ui/broken.txt");
}

TEST(SystemTest, UpdatesMountedElementsWhenLocaleChanges) {
    constexpr char kMultilingualLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {message: Ready}}, "
                                                 "pt: {strings: {message: Pronto}}, "
                                                 "ar: {strings: {message: جاهز}}}\n";

    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot(kMultilingualLocalization));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);
    auto localized = makeElement<HTMLLabelElement>();
    HTMLLabelElement* localizedPtr = localized.get();
    localized->innerHTML(system.t("message"));
    surface->mount(std::move(localized));
    auto probe = std::make_unique<LocaleProbe>();
    LocaleProbe* probePtr = probe.get();
    surface->mount(std::move(probe));

    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(localizedPtr->textContent(), "Pronto");
    EXPECT_EQ(probePtr->notifications(), 2);
}

TEST(SystemTest, SurfaceLocaleSnapshotSkipsDestroyedAndDefersNewRegistrations) {
    constexpr char kLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {}}, pt: {strings: {}}}\n";

    System system;
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot(kLocalization));
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(system.publish(prepared.generation));

    std::unique_ptr<Surface> observer = system.createSurface(fixedTextMetrics());
    std::unique_ptr<Surface> destroyed = system.createSurface(fixedTextMetrics());
    std::unique_ptr<Surface> added;
    bool allowMutation = false;
    const auto observerCounts = std::make_shared<NotificationCounts>();
    const auto destroyedCounts = std::make_shared<NotificationCounts>();
    const auto addedCounts = std::make_shared<NotificationCounts>();

    auto observerProbe = makeElement<SurfaceNotificationProbe>(observerCounts, [&] {
        if (!allowMutation) return;
        destroyed.reset();
        if (!added) {
            added = system.createSurface(fixedTextMetrics());
            added->mount(makeElement<SurfaceNotificationProbe>(addedCounts));
        }
    });
    observer->mount(std::move(observerProbe));
    destroyed->mount(makeElement<SurfaceNotificationProbe>(destroyedCounts));
    observerCounts->armed = true;
    destroyedCounts->armed = true;
    allowMutation = true;

    ASSERT_TRUE(system.setLocale("pt"));
    EXPECT_EQ(observerCounts->locale, 1);
    EXPECT_EQ(destroyedCounts->locale, 0);
    EXPECT_EQ(addedCounts->locale, 0);

    ASSERT_NE(added, nullptr);
    addedCounts->armed = true;
    ASSERT_TRUE(system.setLocale("en"));
    EXPECT_EQ(observerCounts->locale, 2);
    EXPECT_EQ(addedCounts->locale, 1);
}

TEST(SystemTest, SurfaceKeybindingSnapshotSkipsDestroyedAndDefersNewRegistrations) {
    System system;
    std::unique_ptr<Surface> observer = system.createSurface(fixedTextMetrics());
    std::unique_ptr<Surface> destroyed = system.createSurface(fixedTextMetrics());
    std::unique_ptr<Surface> added;
    const auto observerCounts = std::make_shared<NotificationCounts>();
    const auto destroyedCounts = std::make_shared<NotificationCounts>();
    const auto addedCounts = std::make_shared<NotificationCounts>();

    auto observerProbe = makeElement<SurfaceNotificationProbe>(observerCounts, SurfaceNotificationProbe::Callback{}, [&] {
        destroyed.reset();
        if (!added) {
            added = system.createSurface(fixedTextMetrics());
            added->mount(makeElement<SurfaceNotificationProbe>(addedCounts));
        }
    });
    observer->mount(std::move(observerProbe));
    destroyed->mount(makeElement<SurfaceNotificationProbe>(destroyedCounts));
    observerCounts->armed = true;
    destroyedCounts->armed = true;

    system.refreshKeybindings();
    EXPECT_EQ(observerCounts->keybindings, 1);
    EXPECT_EQ(destroyedCounts->keybindings, 0);
    EXPECT_EQ(addedCounts->keybindings, 0);

    ASSERT_NE(added, nullptr);
    addedCounts->armed = true;
    system.refreshKeybindings();
    EXPECT_EQ(observerCounts->keybindings, 2);
    EXPECT_EQ(addedCounts->keybindings, 1);
}

TEST(SystemTest, SurfaceAppearanceSnapshotSkipsDestroyedAndDefersNewRegistrations) {
    System system;
    std::unique_ptr<Surface> observer = system.createSurface(fixedTextMetrics());
    std::unique_ptr<Surface> destroyed = system.createSurface(fixedTextMetrics());
    std::unique_ptr<Surface> added;
    // NativeAppearance::revision is the existing callback seam inside nativeAppearanceChanged.
    auto appearance = std::make_shared<ReentrantNativeAppearance>([&] {
        destroyed.reset();
        if (!added) {
            added = system.createSurface(fixedTextMetrics());
            RecordingPaintContext initialPaint;
            added->paint(initialPaint);
        }
    });

    ASSERT_NE(observer, nullptr);
    ASSERT_NE(destroyed, nullptr);
    system.setNativeAppearance(appearance);

    ASSERT_NE(added, nullptr);
    EXPECT_FALSE(added->needsPaint());
}

TEST(SystemTest, ResolvesDirectionSelectorsWhenLocaleChanges) {
    constexpr char kDirectionStyles[] = "input[switch] { appearance: base; display: inline-grid; width: 44px; height: 20px; } "
                                        "input[switch]::slider-track { grid-area: 1 / 1; width: 100%; } "
                                        "input[switch]::slider-thumb { grid-area: 1 / 1; width: 24px; height: 24px; margin: -2px -1px; } "
                                        "input[switch]:checked::slider-thumb { translate: 22px 0; } "
                                        "input[switch]:dir(rtl):checked::slider-thumb { translate: -22px 0; }";

    const SkinGenerationPrepareResult prepared =
        SkinCompiler().prepare(skinSnapshot("defaultLocale: en\nlocales: {en: {strings: {}}, ar: {strings: {}}}\n", kDirectionStyles));
    ASSERT_TRUE(prepared.ok());

    System system;
    ASSERT_TRUE(system.publish(prepared.generation));
    ASSERT_TRUE(system.setLocale("ar"));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(100.f, 20.f);
    auto control = makeElement<HTMLInputElement>();
    HTMLInputElement* controlPtr = control.get();
    control->type("checkbox").switchMode(true).checked(true);
    control->setRect({10.f, 0.f, 44.f, 20.f});
    surface->mount(std::move(control));
    surface->updateLayout();

    ASSERT_NE(controlPtr->sliderThumb(), nullptr);
    EXPECT_FLOAT_EQ(controlPtr->sliderThumb()->rect().x, -13.f);

    ASSERT_TRUE(system.setLocale("en"));
    surface->updateLayout();
    ASSERT_NE(controlPtr->sliderThumb(), nullptr);
    EXPECT_FLOAT_EQ(controlPtr->sliderThumb()->rect().x, 31.f);
}

TEST(SystemTest, SeparatesPlainTextAndLocalizedContent) {
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales: {en: {strings: {plain: 'hello plain', hello: 'hello <b>world</b>'}}}\n";

    System system;
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot(kLocalization));
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(system.publish(prepared.generation));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);

    auto plain = makeElement<Element>("p");
    Element* plainPtr = plain.get();
    plain->textContent(system.t("hello"));
    surface->mount(std::move(plain));

    ASSERT_EQ(plainPtr->childNodes().size(), 1U);
    ASSERT_NE(plainPtr->childNodes()[0]->asText(), nullptr);
    EXPECT_EQ(plainPtr->textContent(), "hello <b>world</b>");

    auto plainContent = makeElement<Element>("p");
    Element* plainContentPtr = plainContent.get();
    plainContent->innerHTML(system.t("plain"));
    surface->mount(std::move(plainContent));

    ASSERT_EQ(plainContentPtr->childNodes().size(), 1U);
    ASSERT_NE(plainContentPtr->childNodes()[0]->asText(), nullptr);
    EXPECT_EQ(plainContentPtr->textContent(), "hello plain");

    auto rich = makeElement<Element>("p");
    Element* richPtr = rich.get();
    rich->innerHTML(system.t("hello"));
    surface->mount(std::move(rich));

    const NodeList richNodes = richPtr->childNodes();
    ASSERT_EQ(richNodes.size(), 2U);
    ASSERT_NE(richNodes[0]->asText(), nullptr);
    ASSERT_NE(richNodes[1]->asElement(), nullptr);
    EXPECT_EQ(richNodes[0]->asText()->data(), "hello ");
    EXPECT_EQ(richNodes[1]->asElement()->elementName(), "b");
    ASSERT_EQ(richNodes[1]->asElement()->childNodes().size(), 1U);
    ASSERT_NE(richNodes[1]->asElement()->childNodes()[0]->asText(), nullptr);
    EXPECT_EQ(richNodes[1]->asElement()->childNodes()[0]->asText()->data(), "world");
    EXPECT_EQ(richPtr->textContent(), "hello world");

    auto rawContent = makeElement<Element>("p");
    Element* rawContentPtr = rawContent.get();
    rawContent->innerHTML("hello <b>world</b>");
    surface->mount(std::move(rawContent));

    const NodeList rawNodes = rawContentPtr->childNodes();
    ASSERT_EQ(rawNodes.size(), 2U);
    ASSERT_NE(rawNodes[0]->asText(), nullptr);
    ASSERT_NE(rawNodes[1]->asElement(), nullptr);
    EXPECT_EQ(rawNodes[0]->asText()->data(), "hello ");
    EXPECT_EQ(rawNodes[1]->asElement()->elementName(), "b");
    EXPECT_EQ(rawContentPtr->textContent(), "hello world");
}

TEST(SystemTest, RebuildsTranslatedBlockAndControlHTML) {
    constexpr char kLocalization[] = "defaultLocale: en\n"
                                     "locales:\n"
                                     "  en:\n"
                                     "    strings:\n"
                                     "      content: '<div><button>Open</button><p>Ready</p><input type=\"checkbox\" checked></div>'\n"
                                     "  pt:\n"
                                     "    strings:\n"
                                     "      content: '<header><button>Abrir</button><input type=\"checkbox\" switch></header>'\n";

    System system;
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(skinSnapshot(kLocalization));
    ASSERT_TRUE(prepared.ok());
    ASSERT_TRUE(system.publish(prepared.generation));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(240.f, 120.f);
    auto localized = makeElement<Element>("p");
    Element* localizedPtr = localized.get();
    localized->innerHTML(system.t("content"));
    surface->mount(std::move(localized));

    ASSERT_EQ(localizedPtr->children().size(), 1U);
    Element* englishContainer = localizedPtr->children().front();
    ASSERT_EQ(englishContainer->elementName(), "div");
    ASSERT_EQ(englishContainer->children().size(), 3U);
    EXPECT_EQ(englishContainer->children()[0]->elementName(), "button");
    EXPECT_EQ(englishContainer->children()[0]->textContent(), "Open");
    EXPECT_EQ(englishContainer->children()[1]->elementName(), "p");
    EXPECT_EQ(englishContainer->children()[1]->textContent(), "Ready");
    auto* englishInput = dynamic_cast<HTMLInputElement*>(englishContainer->children()[2]);
    ASSERT_NE(englishInput, nullptr);
    EXPECT_TRUE(englishInput->checked());

    ASSERT_TRUE(system.setLocale("pt"));

    ASSERT_EQ(localizedPtr->children().size(), 1U);
    Element* portugueseContainer = localizedPtr->children().front();
    ASSERT_EQ(portugueseContainer->elementName(), "header");
    ASSERT_EQ(portugueseContainer->children().size(), 2U);
    EXPECT_EQ(portugueseContainer->children()[0]->elementName(), "button");
    EXPECT_EQ(portugueseContainer->children()[0]->textContent(), "Abrir");
    auto* portugueseInput = dynamic_cast<HTMLInputElement*>(portugueseContainer->children()[1]);
    ASSERT_NE(portugueseInput, nullptr);
    EXPECT_TRUE(portugueseInput->switchMode());
}

TEST(SystemTest, PreservesLocaleAcrossPublicationAndFallsBackWhenRemoved) {
    constexpr char kMultilingualLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {message: Ready}}, "
                                                 "pt: {strings: {message: Pronto}}, "
                                                 "ar: {strings: {message: جاهز}}}\n";

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
    constexpr char kViewHTML[] = "<p>{{message}}</p>";

    System system;
    const SkinGenerationPrepareResult live = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Old"), kLiveStyles));
    ASSERT_TRUE(live.ok());
    ASSERT_TRUE(system.publish(live.generation));

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);
    auto styled = makeElement<HTMLLabelElement>();
    HTMLLabelElement* styledPtr = styled.get();
    surface->mount(std::move(styled));
    surface->updateLayout();
    EXPECT_FLOAT_EQ(styledPtr->rect().w, 40.f);

    ResourceSnapshot snapshot = skinSnapshot(singleStringLocalization("message", "New"), kCandidateStyles);
    snapshot.add("view.html", kViewHTML);
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());
    EXPECT_EQ(system.generation(), 1ULL);
    EXPECT_EQ(system.resolveText("message"), "Old");

    const ResourceBuildResult candidateBuild = prepared.generation->buildElementTree(ResourceId("view.html"), system.activeLocale());
    ASSERT_TRUE(candidateBuild.ok());
    const Element* candidateText = candidateBuild.rootAs<Element>();
    ASSERT_NE(candidateText, nullptr);
    EXPECT_EQ(candidateText->textContent(), "New");

    ASSERT_TRUE(system.publish(prepared.generation));
    EXPECT_EQ(system.generation(), 2ULL);
    EXPECT_EQ(system.resolveText("message"), "New");
    surface->updateLayout();
    EXPECT_FLOAT_EQ(styledPtr->rect().w, 90.f);

    const ResourceBuildResult liveBuild = system.buildElementTree(ResourceId("view.html"));
    ASSERT_TRUE(liveBuild.ok());
    const Element* liveText = liveBuild.rootAs<Element>();
    ASSERT_NE(liveText, nullptr);
    EXPECT_EQ(liveText->textContent(), "New");
}

TEST(SystemTest, RejectsInvalidUnmountedLayoutResources) {
    constexpr char kStyles[] = "label { width: 90px; }";
    constexpr char kValidHTML[] = "<p>Ready</p>";
    constexpr char kUnsupportedHTML[] = "<unsupported></unsupported>";

    ResourceSnapshot snapshot = skinSnapshot({}, kStyles);
    snapshot.add("valid.html", kValidHTML);
    snapshot.add("unused.html", kUnsupportedHTML);
    const SkinGenerationPrepareResult rejected = SkinCompiler().prepare(std::move(snapshot));

    ASSERT_FALSE(rejected.ok());
    EXPECT_FALSE(rejected.generation);
    ASSERT_FALSE(rejected.errors.empty());
    EXPECT_EQ(rejected.errors.front().code, "layout.element.unknown");
}

TEST(SystemTest, RefreshesKbdPresentationWhenKeybindingsChange) {
    constexpr char kKeybindingLocalization[] = "defaultLocale: en\nlocales: {en: {strings: "
                                               "{fly.label: 'Fly <kbd shortcut=\"toggle-fly\"></kbd>'}}}\n";
    constexpr char kViewHTML[] = "<p>{{fly.label}}</p>";

    ResourceSnapshot snapshot = skinSnapshot(kKeybindingLocalization);
    snapshot.add("view.html", kViewHTML);
    const SkinGenerationPrepareResult prepared = SkinCompiler().prepare(std::move(snapshot));
    ASSERT_TRUE(prepared.ok());

    KeybindingPresentation presentation{{"F"}};
    System system;
    system.setKeybindingResolver(
        [&presentation](const std::string& binding) { return binding == "toggle-fly" ? presentation : KeybindingPresentation{}; });
    ASSERT_TRUE(system.publish(prepared.generation));

    ResourceBuildResult buildResult = system.buildElementTree(ResourceId("view.html"));
    ASSERT_TRUE(buildResult.ok());
    const Element* text = buildResult.rootAs<Element>();
    ASSERT_NE(text, nullptr);

    std::unique_ptr<Surface> surface = system.createSurface(fixedTextMetrics());
    ASSERT_NE(surface, nullptr);
    surface->setViewport(200.f, 100.f);
    surface->mount(*buildResult.document);
    surface->updateLayout();
    EXPECT_EQ(text->textContent(), "Fly F");
    const float initialWidth = text->desiredSize().x;

    presentation = {{"Ctrl", "F"}};
    system.refreshKeybindings();
    surface->updateLayout();
    EXPECT_EQ(text->textContent(), "Fly Ctrl F");
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

TEST(SystemTest, RejectsNestedPublicationMutation) {
    System system;
    const SkinGenerationPrepareResult live = SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Old")));
    ASSERT_TRUE(live.ok());
    ASSERT_TRUE(system.publish(live.generation));

    constexpr char kCandidateLocalization[] = "defaultLocale: en\n"
                                              "locales: {en: {strings: {message: New}}, pt: {strings: {message: Novo}}}\n";
    const SkinGenerationPrepareResult candidate = SkinCompiler().prepare(skinSnapshot(kCandidateLocalization));
    ASSERT_TRUE(candidate.ok());

    class ReentrantCommit final : public PublicationCommit {
    public:
        ReentrantCommit(System& system, std::shared_ptr<const radia::ui::SkinGeneration> candidate)
            : mSystem(system), mCandidate(std::move(candidate)) {}

        bool commit() override {
            nestedPublish = mSystem.publish(mCandidate);
            nestedLocale = mSystem.setLocale("pt");
            return true;
        }

        bool nestedPublish = false;
        bool nestedLocale = false;

    private:
        System& mSystem;
        std::shared_ptr<const radia::ui::SkinGeneration> mCandidate;
    } reentrantCommit(system, candidate.generation);

    ASSERT_TRUE(system.publish(candidate.generation, reentrantCommit));
    EXPECT_FALSE(reentrantCommit.nestedPublish);
    EXPECT_FALSE(reentrantCommit.nestedLocale);
    EXPECT_EQ(system.resolveText("message"), "New");
}
