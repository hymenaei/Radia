/**
 * @file resolver_test.cpp
 * @brief Tests skin manifest resolution, inheritance, resource layering, and diagnostics.
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
#include <algorithm>
#include <chrono>
#include <filesystem>
#include <fstream>
#include "../test/lltut.h"
#include "../test/test.h"
#include "skin/compiler.h"
#include "skin/resolver.h"

namespace tut {
namespace {
void write(const std::filesystem::path& filename, const std::string& source) {
    std::filesystem::create_directories(filename.parent_path());
    std::ofstream output(filename, std::ios::binary);
    output << source;
}

std::string manifest(const std::string& id, const std::string& base, const std::string& radiaRules) {
    return "{\"id\":\"" + id + "\",\"name\":\"Test\",\"author\":\"Test\",\"base\":" + base + ",\"radia\":{" + radiaRules + "}}\n";
}

const std::string kLocalization = "defaultLocale: en\nlocales: {en: {name: English, strings: {title: Base}}}\n";
} // namespace

struct resolverData {
    resolverData() {
        root = std::filesystem::temp_directory_path()
            / ("radia-skin-resolver-" + std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
        std::filesystem::create_directories(root);
    }

    ~resolverData() {
        std::error_code error;
        std::filesystem::remove_all(root, error);
    }

    std::filesystem::path makeRoot(const std::string& directory, const std::string& id) {
        const std::filesystem::path skin = root / directory;
        const char* kResourcePaths =
            "\"stylesheet\": \"radia/skin.radia\",\"layouts\": \"radia/xui\",\"localization\": \"radia/localization.yaml\",\"assets\": \"radia/resources\"";
        write(skin / "manifest.json", manifest(id, "null", kResourcePaths));
        write(skin / "radia/skin.radia", "floater { width: 300px; }");
        write(skin / "radia/localization.yaml", kLocalization);
        write(skin / "radia/xui/base.xml", "<floater/>");
        std::filesystem::create_directories(skin / "radia/resources");
        return skin;
    }

    std::filesystem::path root;
    radia::viewer::ui::SkinResolver resolver;
};
using resolverTest = test_group<resolverData>;
using resolverObject = resolverTest::object;
resolverTest resolverTestCase("RduiSkinResolver");

template<> template<> void resolverObject::test<1>() {
    set_test_name("explicit base chain layers each resource class base-first");
    makeRoot("base", "test.base");
    const std::filesystem::path derived = root / "derived";
    const char* kDerivedResourcePaths =
        "\"stylesheet\": \"radia/derived.radia\",\"layouts\": \"radia/xui\",\"localization\": \"radia/localization.yaml\",\"assets\": \"radia/resources\"";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kDerivedResourcePaths));
    write(derived / "radia/derived.radia", "floater { height: 220px; }");
    write(derived / "radia/localization.yaml", "locales: {en: {strings: {title: Derived}}}\n");
    write(derived / "radia/xui/derived.xml", "<floater title=\"title\"/>");
    const std::string kDerivedAsset = "<svg viewBox=\"0 0 10 10\"><circle cx=\"5\" cy=\"5\" r=\"4\"/></svg>";
    write(derived / "radia/resources/icons/shared.svg", kDerivedAsset);

    const auto result = resolver.resolve(derived, {root});

    ensure("resolved", result.ok());
    ensure_equals("selected ID", result.skinId, std::string("test.derived"));
    ensure("base layout inherited", result.snapshot.load("base.xml").has_value());
    ensure("derived layout added", result.snapshot.load("derived.xml").has_value());
    ensure_equals("styles layered", result.snapshot.layers("skin.radia").size(), std::size_t(2));
    ensure_equals("localizations layered", result.snapshot.layers("localization.yaml").size(), std::size_t(2));
    ensure_equals("derived asset visible", *result.snapshot.load("resources/icons/shared.svg"), kDerivedAsset);

    const auto compiled = radia::ui::SkinCompiler().prepare(result.snapshot);
    ensure("complete generation compiles", compiled.ok());
}

template<> template<> void resolverObject::test<2>() {
    set_test_name("a Skin without an explicit base receives no resources from another root");
    const std::filesystem::path unrelated = makeRoot("unrelated", "test.unrelated");
    write(unrelated / "radia/xui/unrelated.xml", "<floater/>");
    const std::filesystem::path selected = makeRoot("selected", "test.selected");

    const auto result = resolver.resolve(selected, {root});

    ensure("selected root resolves", result.ok());
    ensure("own layout exists", result.snapshot.load("base.xml").has_value());
    ensure("unrelated resources are not consulted", !result.snapshot.load("unrelated.xml").has_value());
    ensure_equals("only one style layer", result.snapshot.layers("skin.radia").size(), std::size_t(1));
}

template<> template<> void resolverObject::test<3>() {
    set_test_name("base cycles reject the complete candidate");
    const std::filesystem::path first = root / "first";
    const std::filesystem::path second = root / "second";
    write(first / "manifest.json", manifest("test.first", "\"test.second\"", "\"layouts\": \"radia/xui\""));
    write(first / "radia/xui/first.xml", "<floater/>");
    write(second / "manifest.json", manifest("test.second", "\"test.first\"", "\"layouts\": \"radia/xui\""));
    write(second / "radia/xui/second.xml", "<floater/>");

    const auto result = resolver.resolve(first, {root});

    ensure("cycle rejected", !result.ok());
    ensure_equals("cycle diagnostic", result.errors.back().code, std::string("skin.base.cycle"));
}

template<> template<> void resolverObject::test<4>() {
    set_test_name("malformed derived layout replaces base layout and rejects compilation");
    const std::filesystem::path base = makeRoot("base", "test.base");
    write(base / "radia/xui/shared.xml", "<floater/>");
    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", "\"layouts\": \"radia/xui\""));
    write(derived / "radia/xui/shared.xml", "<unknown-widget/>");

    const auto resolved = resolver.resolve(derived, {root});
    ensure("filesystem candidate resolves", resolved.ok());
    ensure_equals("derived source replaced base", *resolved.snapshot.load("shared.xml"), std::string("<unknown-widget/>"));

    const auto compiled = radia::ui::SkinCompiler().prepare(resolved.snapshot);
    ensure("malformed override rejects generation", !compiled.ok());
}

template<> template<> void resolverObject::test<5>() {
    set_test_name("duplicate stable IDs reject base lookup");
    makeRoot("base-one", "test.base");
    makeRoot("base-two", "test.base");
    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", "\"layouts\": \"radia/xui\""));
    write(derived / "radia/xui/derived.xml", "<floater/>");

    const auto result = resolver.resolve(derived, {root});

    ensure("duplicate ID rejected", !result.ok());
    ensure_equals("duplicate diagnostic", result.errors.back().code, std::string("skin.id.duplicate"));
}

template<> template<> void resolverObject::test<6>() {
    set_test_name("manifest resource traversal is rejected before discovery");
    const std::filesystem::path selected = root / "selected";
    std::filesystem::create_directories(root / "outside");
    write(selected / "manifest.json", manifest("test.selected", "\"test.base\"", "\"layouts\": \"../outside\""));

    const auto result = resolver.resolve(selected, {root});

    ensure("traversal rejected", !result.ok());
    ensure("stable traversal diagnostic", std::any_of(result.errors.begin(), result.errors.end(), [](const radia::ui::Diagnostic& diagnostic) {
               return diagnostic.code == "skin.manifest.path.traversal";
           }));
}

template<> template<> void resolverObject::test<7>() {
    set_test_name("stylesheet modules are captured inside their owning Skin layer");
    const std::filesystem::path selected = makeRoot("selected", "test.selected");
    write(selected / "radia/skin.radia", "@import \"styles/panel.radia\";\nfloater { height: 220px; }");
    write(selected / "radia/styles/panel.radia", "floater { width: 410px; }");

    const auto resolved = resolver.resolve(selected, {root});

    ensure("Skin with imported module resolves", resolved.ok());
    const auto& layers = resolved.snapshot.layers("skin.radia");
    ensure_equals("one stylesheet layer", layers.size(), std::size_t(1));
    ensure_equals("entrypoint identity is Skin-relative", layers.front().entrypoint, std::string("radia/skin.radia"));
    ensure("imported module captured", layers.front().modules.contains("radia/styles/panel.radia"));
    ensure("complete generation with import compiles", radia::ui::SkinCompiler().prepare(resolved.snapshot).ok());
}

template<> template<> void resolverObject::test<8>() {
    set_test_name("a derived stylesheet import never falls through to a Base Skin module");
    const std::filesystem::path base = makeRoot("base", "test.base");
    write(base / "radia/skin.radia", "@import \"shared.radia\";");
    write(base / "radia/shared.radia", "floater { width: 300px; }");

    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", "\"stylesheet\": \"radia/skin.radia\""));
    write(derived / "radia/skin.radia", "@import \"shared.radia\";");

    const auto resolved = resolver.resolve(derived, {root});
    ensure("filesystem snapshot resolves", resolved.ok());

    const auto compiled = radia::ui::SkinCompiler().prepare(resolved.snapshot);
    ensure("missing derived module rejects generation", !compiled.ok());
    ensure_equals("no Base module fallback diagnostic", compiled.errors.front().code, std::string("stylesheet.import.missing"));
    ensure_equals("diagnostic identifies derived entrypoint", compiled.errors.front().source, std::string("test.derived/radia/skin.radia"));
}

template<> template<> void resolverObject::test<9>() {
    set_test_name("bundled Skin compiles its real imported variant module");
    const std::filesystem::path bundled = std::filesystem::path(tut::sSourceDir) / "skins/default";

    const auto resolved = resolver.resolve(bundled, {bundled.parent_path()});

    ensure("bundled Skin resolves", resolved.ok());
    const auto& layers = resolved.snapshot.layers("skin.radia");
    ensure_equals("bundled Skin has one stylesheet layer", layers.size(), std::size_t(1));
    ensure("variant module is captured", layers.front().modules.contains("radia/variants.radia"));
    const auto compiled = radia::ui::SkinCompiler().prepare(resolved.snapshot);
    const std::string failure = compiled.errors.empty() ? std::string("no diagnostic") : compiled.errors.front().formatted();
    ensure("bundled Skin Generation compiles: " + failure, compiled.ok());
}

template<> template<> void resolverObject::test<10>() {
    set_test_name("bundled Skin keeps its stable Skin and logical resource identities");
    const std::filesystem::path bundled = std::filesystem::path(tut::sSourceDir) / "skins/default";

    const auto resolved = resolver.resolve(bundled, {bundled.parent_path()});

    ensure("bundled Skin resolves", resolved.ok());
    ensure_equals("bundled stable Skin ID", resolved.skinId, std::string("alchemy.default"));

    const auto& styleLayers = resolved.snapshot.layers("skin.radia");
    ensure_equals("bundled Skin has one stylesheet layer", styleLayers.size(), std::size_t(1));
    ensure_equals("stylesheet source identity", styleLayers.front().sourceName, std::string("alchemy.default/radia/skin.radia"));
    ensure_equals("stylesheet entrypoint identity", styleLayers.front().entrypoint, std::string("radia/skin.radia"));

    const auto& localizationLayers = resolved.snapshot.layers("localization.yaml");
    ensure_equals("bundled Skin has one localization layer", localizationLayers.size(), std::size_t(1));
    ensure_equals("localization source identity", localizationLayers.front().sourceName, std::string("alchemy.default/radia/localization.yaml"));

    ensure("demo Floater keeps its logical layout identity", resolved.snapshot.load("floater_demo.xml").has_value());
    ensure("Floater defaults keep their logical layout identity", resolved.snapshot.load("widgets/floater.xml").has_value());
    ensure("search icon keeps its logical asset identity", resolved.snapshot.load("resources/icons/search.svg").has_value());
    ensure("close icon keeps its logical asset identity", resolved.snapshot.load("resources/icons/close.svg").has_value());
    ensure("minimize icon keeps its logical asset identity", resolved.snapshot.load("resources/icons/minimize.svg").has_value());

    const auto compiled = radia::ui::SkinCompiler().prepare(resolved.snapshot);
    const std::string failure = compiled.errors.empty() ? std::string("no diagnostic") : compiled.errors.front().formatted();
    ensure("identified bundled resources compile as one Skin Generation: " + failure, compiled.ok());
}
} // namespace tut
