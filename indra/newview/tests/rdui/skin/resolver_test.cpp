/**
 * @file resolver_test.cpp
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

std::string manifest(const std::string& id, const std::string& base, const std::string& radia) {
    return "{\n"
           "  \"id\": \""
        + id
        + "\",\n"
          "  \"name\": \"Test\",\n"
          "  \"author\": \"Test\",\n"
          "  \"base\": "
        + base
        + ",\n"
          "  \"radia\": {"
        + radia
        + "}\n"
          "}\n";
}

const std::string LOCALIZATION = R"YAML(defaultLocale: en
locales:
  en:
    name: English
    strings:
      title: Base
)YAML";
} // namespace

struct skin_resolver {
    skin_resolver() {
        root = std::filesystem::temp_directory_path()
            / ("rdui-skin-resolver-" + std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
        std::filesystem::create_directories(root);
    }

    ~skin_resolver() {
        std::error_code error;
        std::filesystem::remove_all(root, error);
    }

    std::filesystem::path makeRoot(const std::string& directory, const std::string& id) {
        const std::filesystem::path skin = root / directory;
        write(skin / "manifest.json",
              manifest(id, "null",
                       "\"stylesheet\": \"rdui/skin.radia\","
                       "\"layouts\": \"rdui/xui\","
                       "\"localization\": \"rdui/localization.yaml\","
                       "\"assets\": \"rdui/resources\""));
        write(skin / "rdui/skin.radia", "floater { width: 300px; }");
        write(skin / "rdui/localization.yaml", LOCALIZATION);
        write(skin / "rdui/xui/base.xml", "<floater/>");
        std::filesystem::create_directories(skin / "rdui/resources");
        return skin;
    }

    std::filesystem::path root;
    rdui::viewer::SkinResolver resolver;
};

using skin_resolver_group = test_group<skin_resolver>;
using skin_resolver_object = skin_resolver_group::object;
skin_resolver_group skin_resolver_tests("RduiSkinResolver");

template<> template<> void skin_resolver_object::test<1>() {
    set_test_name("explicit base chain layers each resource class base-first");
    makeRoot("base", "test.base");
    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json",
          manifest("test.derived", "\"test.base\"",
                   "\"stylesheet\": \"rdui/derived.radia\","
                   "\"layouts\": \"rdui/xui\","
                   "\"localization\": \"rdui/localization.yaml\","
                   "\"assets\": \"rdui/resources\""));
    write(derived / "rdui/derived.radia", "floater { height: 220px; }");
    write(derived / "rdui/localization.yaml", R"YAML(
locales:
  en:
    strings:
      title: Derived
)YAML");
    write(derived / "rdui/xui/derived.xml", "<floater title=\"title\"/>");
    const std::string derived_asset = "<svg viewBox=\"0 0 10 10\"><circle cx=\"5\" cy=\"5\" r=\"4\"/></svg>";
    write(derived / "rdui/resources/icons/shared.svg", derived_asset);

    const auto result = resolver.resolve(derived, {root});

    ensure("resolved", result.ok());
    ensure_equals("selected ID", result.skin_id, std::string("test.derived"));
    ensure("base layout inherited", result.snapshot.load("base.xml").has_value());
    ensure("derived layout added", result.snapshot.load("derived.xml").has_value());
    ensure_equals("styles layered", result.snapshot.layers("skin.radia").size(), std::size_t(2));
    ensure_equals("localizations layered", result.snapshot.layers("localization.yaml").size(), std::size_t(2));
    ensure_equals("derived asset visible", *result.snapshot.load("resources/icons/shared.svg"), derived_asset);

    const auto compiled = rdui::SkinCompiler().prepare(result.snapshot);
    ensure("complete generation compiles", compiled.ok());
}

template<> template<> void skin_resolver_object::test<2>() {
    set_test_name("a Skin without an explicit base receives no resources from another root");
    const std::filesystem::path unrelated = makeRoot("unrelated", "test.unrelated");
    write(unrelated / "rdui/xui/unrelated.xml", "<floater/>");
    const std::filesystem::path selected = makeRoot("selected", "test.selected");

    const auto result = resolver.resolve(selected, {root});

    ensure("selected root resolves", result.ok());
    ensure("own layout exists", result.snapshot.load("base.xml").has_value());
    ensure("unrelated resources are not consulted", !result.snapshot.load("unrelated.xml").has_value());
    ensure_equals("only one style layer", result.snapshot.layers("skin.radia").size(), std::size_t(1));
}

template<> template<> void skin_resolver_object::test<3>() {
    set_test_name("base cycles reject the complete candidate");
    const std::filesystem::path first = root / "first";
    const std::filesystem::path second = root / "second";
    write(first / "manifest.json", manifest("test.first", "\"test.second\"", "\"layouts\": \"rdui/xui\""));
    write(first / "rdui/xui/first.xml", "<floater/>");
    write(second / "manifest.json", manifest("test.second", "\"test.first\"", "\"layouts\": \"rdui/xui\""));
    write(second / "rdui/xui/second.xml", "<floater/>");

    const auto result = resolver.resolve(first, {root});

    ensure("cycle rejected", !result.ok());
    ensure_equals("cycle diagnostic", result.errors.back().code, std::string("skin.base.cycle"));
}

template<> template<> void skin_resolver_object::test<4>() {
    set_test_name("malformed derived layout replaces base layout and rejects compilation");
    const std::filesystem::path base = makeRoot("base", "test.base");
    write(base / "rdui/xui/shared.xml", "<floater/>");
    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", "\"layouts\": \"rdui/xui\""));
    write(derived / "rdui/xui/shared.xml", "<unknown-widget/>");

    const auto resolved = resolver.resolve(derived, {root});
    ensure("filesystem candidate resolves", resolved.ok());
    ensure_equals("derived source replaced base", *resolved.snapshot.load("shared.xml"), std::string("<unknown-widget/>"));

    const auto compiled = rdui::SkinCompiler().prepare(resolved.snapshot);
    ensure("malformed override rejects generation", !compiled.ok());
}

template<> template<> void skin_resolver_object::test<5>() {
    set_test_name("duplicate stable IDs reject base lookup");
    makeRoot("base-one", "test.base");
    makeRoot("base-two", "test.base");
    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", "\"layouts\": \"rdui/xui\""));
    write(derived / "rdui/xui/derived.xml", "<floater/>");

    const auto result = resolver.resolve(derived, {root});

    ensure("duplicate ID rejected", !result.ok());
    ensure_equals("duplicate diagnostic", result.errors.back().code, std::string("skin.id.duplicate"));
}

template<> template<> void skin_resolver_object::test<6>() {
    set_test_name("manifest resource traversal is rejected before discovery");
    const std::filesystem::path selected = root / "selected";
    std::filesystem::create_directories(root / "outside");
    write(selected / "manifest.json", manifest("test.selected", "\"test.base\"", "\"layouts\": \"../outside\""));

    const auto result = resolver.resolve(selected, {root});

    ensure("traversal rejected", !result.ok());
    ensure("stable traversal diagnostic", std::any_of(result.errors.begin(), result.errors.end(), [](const rdui::Diagnostic& diagnostic) {
               return diagnostic.code == "skin.manifest.path.traversal";
           }));
}

template<> template<> void skin_resolver_object::test<7>() {
    set_test_name("stylesheet modules are captured inside their owning Skin layer");
    const std::filesystem::path selected = makeRoot("selected", "test.selected");
    write(selected / "rdui/skin.radia", "@import \"styles/panel.radia\";\nfloater { height: 220px; }");
    write(selected / "rdui/styles/panel.radia", "floater { width: 410px; }");

    const auto resolved = resolver.resolve(selected, {root});

    ensure("Skin with imported module resolves", resolved.ok());
    const auto& layers = resolved.snapshot.layers("skin.radia");
    ensure_equals("one stylesheet layer", layers.size(), std::size_t(1));
    ensure_equals("entrypoint identity is Skin-relative", layers.front().entrypoint, std::string("rdui/skin.radia"));
    ensure("imported module captured", layers.front().modules.contains("rdui/styles/panel.radia"));
    ensure("complete generation with import compiles", rdui::SkinCompiler().prepare(resolved.snapshot).ok());
}

template<> template<> void skin_resolver_object::test<8>() {
    set_test_name("a derived stylesheet import never falls through to a Base Skin module");
    const std::filesystem::path base = makeRoot("base", "test.base");
    write(base / "rdui/skin.radia", "@import \"shared.radia\";");
    write(base / "rdui/shared.radia", "floater { width: 300px; }");

    const std::filesystem::path derived = root / "derived";
    write(derived / "manifest.json", manifest("test.derived", "\"test.base\"", "\"stylesheet\": \"rdui/skin.radia\""));
    write(derived / "rdui/skin.radia", "@import \"shared.radia\";");

    const auto resolved = resolver.resolve(derived, {root});
    ensure("filesystem snapshot resolves", resolved.ok());

    const auto compiled = rdui::SkinCompiler().prepare(resolved.snapshot);
    ensure("missing derived module rejects generation", !compiled.ok());
    ensure_equals("no Base module fallback diagnostic", compiled.errors.front().code, std::string("stylesheet.import.missing"));
    ensure_equals("diagnostic identifies derived entrypoint", compiled.errors.front().source, std::string("test.derived/rdui/skin.radia"));
}

template<> template<> void skin_resolver_object::test<9>() {
    set_test_name("bundled Skin compiles its real imported variant module");
    const std::filesystem::path bundled = std::filesystem::path(tut::sSourceDir) / "skins/default";

    const auto resolved = resolver.resolve(bundled, {bundled.parent_path()});

    ensure("bundled Skin resolves", resolved.ok());
    const auto& layers = resolved.snapshot.layers("skin.radia");
    ensure_equals("bundled Skin has one stylesheet layer", layers.size(), std::size_t(1));
    ensure("variant module is captured", layers.front().modules.contains("rdui/variants.radia"));
    const auto compiled = rdui::SkinCompiler().prepare(resolved.snapshot);
    const std::string failure = compiled.errors.empty() ? std::string("no diagnostic") : compiled.errors.front().formatted();
    ensure("bundled Skin Generation compiles: " + failure, compiled.ok());
}

template<> template<> void skin_resolver_object::test<10>() {
    set_test_name("bundled Skin keeps its stable Skin and logical resource identities");
    const std::filesystem::path bundled = std::filesystem::path(tut::sSourceDir) / "skins/default";

    const auto resolved = resolver.resolve(bundled, {bundled.parent_path()});

    ensure("bundled Skin resolves", resolved.ok());
    ensure_equals("bundled stable Skin ID", resolved.skin_id, std::string("alchemy.default"));

    const auto& style_layers = resolved.snapshot.layers("skin.radia");
    ensure_equals("bundled Skin has one stylesheet layer", style_layers.size(), std::size_t(1));
    ensure_equals("stylesheet source identity", style_layers.front().source_name, std::string("alchemy.default/rdui/skin.radia"));
    ensure_equals("stylesheet entrypoint identity", style_layers.front().entrypoint, std::string("rdui/skin.radia"));

    const auto& localization_layers = resolved.snapshot.layers("localization.yaml");
    ensure_equals("bundled Skin has one localization layer", localization_layers.size(), std::size_t(1));
    ensure_equals("localization source identity", localization_layers.front().source_name, std::string("alchemy.default/rdui/localization.yaml"));

    ensure("demo Floater keeps its logical layout identity", resolved.snapshot.load("floater_demo.xml").has_value());
    ensure("Floater defaults keep their logical layout identity", resolved.snapshot.load("widgets/floater.xml").has_value());
    ensure("search icon keeps its logical asset identity", resolved.snapshot.load("resources/icons/search.svg").has_value());
    ensure("close icon keeps its logical asset identity", resolved.snapshot.load("resources/icons/close.svg").has_value());
    ensure("minimize icon keeps its logical asset identity", resolved.snapshot.load("resources/icons/minimize.svg").has_value());

    const auto compiled = rdui::SkinCompiler().prepare(resolved.snapshot);
    const std::string failure = compiled.errors.empty() ? std::string("no diagnostic") : compiled.errors.front().formatted();
    ensure("identified bundled resources compile as one Skin Generation: " + failure, compiled.ok());
}
} // namespace tut
