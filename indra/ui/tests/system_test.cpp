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
#include <utility>
#include "../test/lltut.h"
#include "skin/compiler.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"
#include "widgets/label.h"
#include "widgets/text.h"

namespace tut {
namespace {
const char* kEmptyLocalization = "defaultLocale: en\nlocales: {en: {name: English, strings: {}}}\n";

std::string singleStringLocalization(const std::string& key, const std::string& value) {
    return "defaultLocale: en\nlocales: {en: {name: English, strings: {" + key + ": \"" + value + "\"}}}\n";
}

radia::ui::ResourceSnapshot skinSnapshot(std::string localization = kEmptyLocalization, std::string style = {}) {
    if (localization.empty()) localization = kEmptyLocalization;
    radia::ui::ResourceSnapshot snapshot;
    snapshot.add("localization.yaml", std::move(localization));
    snapshot.add("skin.radia", std::move(style));
    return snapshot;
}

float resolvedLabelWidth(const radia::ui::System& system) {
    std::unique_ptr<radia::ui::Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    surface->setViewport(200.f, 100.f);
    auto label = std::make_unique<radia::ui::Label>();
    radia::ui::Label* labelPtr = label.get();
    surface->mount(std::move(label));
    surface->updateLayout();
    return labelPtr->rect().w;
}
} // namespace

class LocaleProbe final : public radia::ui::Widget {
public:
    LocaleProbe() : Widget("locale-probe") {}
    int notifications() const { return mNotifications; }

private:
    void onLocaleChanged(const radia::ui::System&) override { ++mNotifications; }
    int mNotifications = 0;
};

struct systemData {};
using systemTest = test_group<systemData>;
using systemObject = systemTest::object;
systemTest systemTestCase("system");

template<> template<> void systemObject::test<1>() {
    radia::ui::ResourceSnapshot snapshot = skinSnapshot(singleStringLocalization("message", "Ready"), "label { width: 40px; }");
    snapshot.add("view.xml", "<text id=\"message\">message</text>");
    snapshot.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>");

    const radia::ui::SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ensure("complete generation prepares", prepared.ok());

    radia::ui::System system;
    system.publish(prepared.generation);
    ensure_equals("publication advances generation", system.generation(), 1ULL);
    ensure_equals("localization published", system.resolveText("message"), "Ready");
    ensure_equals("stylesheet published", resolvedLabelWidth(system), 40.f);
    ensure("compiled icon published", system.hasIcon("search"));

    radia::ui::LayoutBuildResult buildResult = system.buildWidgetTree("view.xml");
    ensure("System creates localized Widget tree", buildResult.ok());
    ensure_equals("Widget tree uses published localization", buildResult.rootAs<radia::ui::Text>()->text(), "Ready");

    std::unique_ptr<radia::ui::Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    ensure("System creates Surface", surface != nullptr);
    surface->setViewport(100.f, 100.f);
    auto styled = std::make_unique<radia::ui::Label>();
    radia::ui::Label* styledPtr = styled.get();
    surface->mount(std::move(styled));
    surface->updateLayout();
    ensure_equals("Surface shares published stylesheet", styledPtr->rect().w, 40.f);
}

template<> template<> void systemObject::test<2>() {
    radia::ui::System system;
    radia::ui::SkinGenerationPrepareResult live =
        radia::ui::SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Live"), "label { width: 40px; }"));
    ensure("live generation prepares", live.ok());
    system.publish(live.generation);

    const radia::ui::SkinGenerationPrepareResult rejected =
        radia::ui::SkinCompiler().prepare(skinSnapshot("defaultLocale: [", "label { flow: sideways; width: 90px; }"));
    ensure("invalid candidate rejected", !rejected.ok());
    ensure_equals("failed preparation preserves generation", system.generation(), 1ULL);
    ensure_equals("failed preparation preserves localization", system.resolveText("message"), "Live");
    ensure_equals("failed preparation preserves stylesheet", resolvedLabelWidth(system), 40.f);
}

template<> template<> void systemObject::test<3>() {
    radia::ui::ResourceSnapshot snapshot = skinSnapshot({}, "icon { size: 16px; }");
    snapshot.add("resources/icons/search.svg", "");
    const radia::ui::SkinGenerationPrepareResult rejected = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ensure("empty icon rejects complete generation", !rejected.ok());
    ensure("rejected generation is not exposed", rejected.generation == nullptr);
}

template<> template<> void systemObject::test<4>() {
    radia::ui::ResourceSnapshot snapshot = skinSnapshot({}, "icon { size: 16px; }");
    snapshot.add("known.xml", "<icon src=\"actions/search\"/>");
    snapshot.add("missing.xml", "<icon src=\"actions/missing\"/>");
    snapshot.add("resources/icons/actions/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10 10\"/></svg>");

    const radia::ui::SkinGenerationPrepareResult rejected = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ensure("unknown icon in any Layout Resource rejects generation", !rejected.ok());
    ensure("invalid complete generation is not exposed", rejected.generation == nullptr);
}

template<> template<> void systemObject::test<5>() {
    radia::ui::ResourceSnapshot missingLocalization;
    missingLocalization.add("skin.radia", "label { width: 40px; }");
    const radia::ui::SkinGenerationPrepareResult rejected = radia::ui::SkinCompiler().prepare(std::move(missingLocalization));
    ensure("missing required resource rejects candidate", !rejected.ok());
    ensure_equals("diagnostic identifies missing resource", rejected.errors.front().source, "localization.yaml");
}

template<> template<> void systemObject::test<6>() {
    radia::ui::ResourceSnapshot snapshot = skinSnapshot({}, "icon { size: 16px; }");
    snapshot.add("resources/icons/search.svg", "<svg viewBox=\"0 0 24 24\"><path d=\"M0 0 L10\"/></svg>");
    const radia::ui::SkinGenerationPrepareResult rejected = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ensure("malformed SVG rejects complete generation", !rejected.ok());
    ensure_equals("icon diagnostic identifies resource", rejected.errors.front().source, "resources/icons/search.svg");
}

template<> template<> void systemObject::test<7>() {
    const std::string kMultilingual =
        "defaultLocale: en\nlocales: {en: {name: English, strings: {message: Ready}}, pt: {name: Português, strings: {message: Pronto}}, ar: {name: العربية, direction: rtl, strings: {message: جاهز}}}\n";
    radia::ui::SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(skinSnapshot(kMultilingual));
    ensure("multilingual generation prepares", prepared.ok());

    radia::ui::System system;
    system.publish(prepared.generation);
    std::unique_ptr<radia::ui::Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    auto localized = std::make_unique<radia::ui::Label>();
    radia::ui::Label* localizedPtr = localized.get();
    localized->setContent(system.localize("message"));
    surface->root().addChild(std::move(localized));
    auto probe = std::make_unique<LocaleProbe>();
    LocaleProbe* probePtr = probe.get();
    surface->root().addChild(std::move(probe));

    ensure("Portuguese locale selected", system.setLocale("pt"));
    ensure_equals("localized Widget updates reactively", localizedPtr->text(), "Pronto");
    ensure_equals("Surface delivers locale change", probePtr->notifications(), 2);

    radia::ui::SkinGenerationPrepareResult compatible = radia::ui::SkinCompiler().prepare(skinSnapshot(kMultilingual));
    system.publish(compatible.generation);
    ensure_equals("selected locale survives publication", system.activeLocale(), "pt");

    radia::ui::SkinGenerationPrepareResult fallback = radia::ui::SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Ready again")));
    system.publish(fallback.generation);
    ensure_equals("removed locale falls back to default", system.activeLocale(), "en");
    ensure_equals("fallback refreshes localized Widget", localizedPtr->text(), "Ready again");
}

template<> template<> void systemObject::test<8>() {
    radia::ui::System system;
    radia::ui::SkinGenerationPrepareResult live =
        radia::ui::SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Old"), "label { width: 40px; }"));
    system.publish(live.generation);
    std::unique_ptr<radia::ui::Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    surface->setViewport(200.f, 100.f);
    auto styled = std::make_unique<radia::ui::Label>();
    radia::ui::Label* styledPtr = styled.get();
    surface->mount(std::move(styled));
    surface->updateLayout();
    ensure_equals("existing Surface starts with live stylesheet", styledPtr->rect().w, 40.f);

    radia::ui::ResourceSnapshot snapshot = skinSnapshot(singleStringLocalization("message", "New"), "label { width: 90px; }");
    snapshot.add("view.xml", "<text>message</text>");
    radia::ui::SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ensure("candidate generation prepares", prepared.ok());
    ensure_equals("preparation does not advance live generation", system.generation(), 1ULL);
    ensure_equals("preparation preserves live localization", system.resolveText("message"), "Old");
    radia::ui::LayoutBuildResult candidateBuildResult = prepared.generation->buildWidgetTree("view.xml", system.activeLocale());
    ensure("candidate Widget tree builds against candidate generation", candidateBuildResult.ok());
    ensure_equals("candidate Widget tree uses candidate localization", candidateBuildResult.rootAs<radia::ui::Text>()->text(), "New");

    system.publish(prepared.generation);
    ensure_equals("publication advances generation once", system.generation(), 2ULL);
    ensure_equals("publication commits candidate localization", system.resolveText("message"), "New");
    surface->updateLayout();
    ensure_equals("existing Surface observes published stylesheet", styledPtr->rect().w, 90.f);
    ensure("live Widget tree creation uses published snapshot", system.buildWidgetTree("view.xml").ok());
}

template<> template<> void systemObject::test<9>() {
    radia::ui::System system;
    radia::ui::SkinGenerationPrepareResult live = radia::ui::SkinCompiler().prepare(skinSnapshot({}, "label { width: 40px; }"));
    system.publish(live.generation);

    const radia::ui::SkinGenerationPrepareResult rejected = radia::ui::SkinCompiler().prepare(skinSnapshot("defaultLocale: [", "label { width: 90px; }"));
    ensure("invalid prepared generation rejects", !rejected.ok());
    ensure_equals("rejected preparation preserves generation", system.generation(), 1ULL);
    ensure_equals("rejected preparation preserves stylesheet", resolvedLabelWidth(system), 40.f);
}

template<> template<> void systemObject::test<10>() {
    radia::ui::ResourceSnapshot invalid = skinSnapshot({}, "label { width: 90px; }");
    invalid.add("valid.xml", "<text>Ready</text>");
    invalid.add("unused.xml", "<unsupported/>");

    const radia::ui::SkinGenerationPrepareResult rejected = radia::ui::SkinCompiler().prepare(std::move(invalid));
    ensure("invalid unmounted Layout Resource rejects complete generation", !rejected.ok());
    ensure("rejected complete generation is not exposed", rejected.generation == nullptr);
}

template<> template<> void systemObject::test<11>() {
    radia::ui::ResourceSnapshot snapshot =
        skinSnapshot("defaultLocale: en\nlocales: {en: {name: English, strings: {fly.label: 'Fly <kbd shortcut=\"toggle-fly\"/>'}}}\n");
    snapshot.add("view.xml", "<text>fly.label</text>");
    radia::ui::SkinGenerationPrepareResult prepared = radia::ui::SkinCompiler().prepare(std::move(snapshot));
    ensure("Kbd presentation fixture prepares", prepared.ok());

    radia::ui::KeybindingPresentation presentation{{"F"}};
    radia::ui::System system;
    system.setKeybindingResolver(
        [&presentation](const std::string& binding) { return binding == "toggle-fly" ? presentation : radia::ui::KeybindingPresentation{}; });
    system.publish(prepared.generation);
    radia::ui::LayoutBuildResult buildResult = system.buildWidgetTree("view.xml");
    auto* text = buildResult.rootAs<radia::ui::Text>();
    ensure("Kbd Widget tree builds", buildResult.ok() && text);

    std::unique_ptr<radia::ui::Surface> surface = system.createSurface(radia::ui::fixedTextMetrics());
    surface->setViewport(200.f, 100.f);
    surface->mount(std::move(buildResult.root));
    surface->updateLayout();
    ensure_equals("Kbd resolves through the System presentation seam", text->text(), "Fly F");
    const float initialWidth = text->desiredSize().x;

    presentation = {{"Ctrl", "F"}};
    system.refreshKeybindings();
    surface->updateLayout();
    ensure_equals("Kbd refreshes after a user keybinding change", text->text(), "Fly Ctrl F");
    ensure("changed Kbd presentation invalidates intrinsic measurement", text->desiredSize().x > initialWidth);
}

template<> template<> void systemObject::test<12>() {
    radia::ui::System system;
    radia::ui::SkinGenerationPrepareResult live = radia::ui::SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "Old")));
    system.publish(live.generation);

    radia::ui::SkinGenerationPrepareResult candidate = radia::ui::SkinCompiler().prepare(skinSnapshot(singleStringLocalization("message", "New")));
    bool callbackSawCandidate = false;
    class RejectPublication final : public radia::ui::PublicationCommit {
    public:
        RejectPublication(radia::ui::System& system, bool& sawCandidate) : mSystem(system), mSawCandidate(sawCandidate) {}

        bool commit() override {
            mSawCandidate = mSystem.resolveText("message") == "New";
            return false;
        }

    private:
        radia::ui::System& mSystem;
        bool& mSawCandidate;
    } rejectPublication(system, callbackSawCandidate);
    const bool committed = system.publish(std::move(candidate.generation), rejectPublication);

    ensure("transactional publication reports callback rejection", !committed);
    ensure("publication callback observes candidate generation", callbackSawCandidate);
    ensure_equals("rejected transactional publication preserves generation", system.generation(), 1ULL);
    ensure_equals("rejected transactional publication restores localization", system.resolveText("message"), "Old");
}
} // namespace tut
