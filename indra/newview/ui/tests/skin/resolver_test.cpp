/**
 * @file resolver_test.cpp
 * @brief Tests Skin manifest resolution, inheritance, resource layering, and diagnostics.
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
#include <gtest/gtest.h>
#include <stdexcept>
#include <string_view>
#include "skin/compiler.h"
#include "skin/resolver.h"

namespace {
using radia::ui::SkinCompiler;
using radia::viewer::ui::SkinResolver;

constexpr char kRootResourcePaths[] =
    R"("stylesheet": "radia/skin.radia","layouts": "radia/xui","localization": "radia/localization.yaml","assets": "radia/resources")";
constexpr char kLayoutResourcePath[] = R"("layouts": "radia/xui")";
constexpr char kLocalization[] = "defaultLocale: en\nlocales: {en: {name: English, strings: {title: Base}}}\n";

void writeFile(const std::filesystem::path& filename, std::string_view source) {
    std::filesystem::create_directories(filename.parent_path());
    std::ofstream output(filename, std::ios::binary);
    if (!output) throw std::runtime_error("Could not create test file: " + filename.string());
    output << source;
    if (!output) throw std::runtime_error("Could not write test file: " + filename.string());
}

std::string manifest(std::string_view id, std::string_view base, std::string_view resources) {
    return "{\"id\":\""
        + std::string(id)
        + "\",\"name\":\"Test\",\"author\":\"Test\",\"base\":"
        + std::string(base)
        + ",\"radia\":{"
        + std::string(resources)
        + "}}\n";
}

class SkinResolverTest : public ::testing::Test {
protected:
    void SetUp() override {
        root = std::filesystem::temp_directory_path()
            / ("radia-skin-resolver-" + std::to_string(std::chrono::steady_clock::now().time_since_epoch().count()));
        std::filesystem::create_directories(root);
    }

    void TearDown() override {
        std::error_code error;
        std::filesystem::remove_all(root, error);
    }

    std::filesystem::path makeRoot(std::string_view directory, std::string_view id) {
        const std::filesystem::path skin = root / directory;
        writeFile(skin / "manifest.json", manifest(id, "null", kRootResourcePaths));
        writeFile(skin / "radia/skin.radia", "floater { width: 300px; }");
        writeFile(skin / "radia/localization.yaml", kLocalization);
        writeFile(skin / "radia/xui/base.xml", "<floater/>");
        std::filesystem::create_directories(skin / "radia/resources");
        return skin;
    }

    std::filesystem::path viewerSourceRoot() const { return std::filesystem::path(__FILE__).parent_path().parent_path().parent_path().parent_path(); }

    std::filesystem::path root;
    SkinResolver resolver;
};
} // namespace

TEST_F(SkinResolverTest, LayersDerivedResourcesAfterTheirBase) {
    makeRoot("base", "test.base");
    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kRootResourcePaths));
    writeFile(derived / "radia/skin.radia", "floater { height: 220px; }");
    writeFile(derived / "radia/localization.yaml", "locales: {en: {strings: {title: Derived}}}\n");
    writeFile(derived / "radia/xui/derived.xml", "<floater title=\"title\"/>");

    constexpr char kDerivedAsset[] = R"(<svg viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>)";
    writeFile(derived / "radia/resources/icons/shared.svg", kDerivedAsset);

    const auto result = resolver.resolve(derived, {root});

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.skinId, "test.derived");
    EXPECT_TRUE(result.snapshot.load("base.xml").has_value());
    EXPECT_TRUE(result.snapshot.load("derived.xml").has_value());
    EXPECT_EQ(result.snapshot.layers("skin.radia").size(), std::size_t{2});
    EXPECT_EQ(result.snapshot.layers("localization.yaml").size(), std::size_t{2});

    const auto asset = result.snapshot.load("resources/icons/shared.svg");
    ASSERT_TRUE(asset.has_value());
    EXPECT_EQ(*asset, kDerivedAsset);
    EXPECT_TRUE(SkinCompiler().prepare(result.snapshot).ok());
}

TEST_F(SkinResolverTest, DoesNotMergeResourcesFromUnrelatedSkins) {
    makeRoot("unrelated", "test.unrelated");
    writeFile(root / "unrelated/radia/xui/unrelated.xml", "<floater/>");
    const std::filesystem::path selected = makeRoot("selected", "test.selected");

    const auto result = resolver.resolve(selected, {root});

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.snapshot.load("base.xml").has_value());
    EXPECT_FALSE(result.snapshot.load("unrelated.xml").has_value());
    EXPECT_EQ(result.snapshot.layers("skin.radia").size(), std::size_t{1});
}

TEST_F(SkinResolverTest, RejectsBaseSkinCycles) {
    const std::filesystem::path first = root / "first";
    const std::filesystem::path second = root / "second";
    writeFile(first / "manifest.json", manifest("test.first", "\"test.second\"", kLayoutResourcePath));
    writeFile(first / "radia/xui/first.xml", "<floater/>");
    writeFile(second / "manifest.json", manifest("test.second", "\"test.first\"", kLayoutResourcePath));
    writeFile(second / "radia/xui/second.xml", "<floater/>");

    const auto result = resolver.resolve(first, {root});

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.back().code, "skin.base.cycle");
}

TEST_F(SkinResolverTest, RejectsMalformedDerivedLayoutOverrides) {
    const std::filesystem::path base = makeRoot("base", "test.base");
    writeFile(base / "radia/xui/shared.xml", "<floater/>");
    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kLayoutResourcePath));
    writeFile(derived / "radia/xui/shared.xml", "<unknown-widget/>");

    const auto resolved = resolver.resolve(derived, {root});

    ASSERT_TRUE(resolved.ok());
    const auto shared = resolved.snapshot.load("shared.xml");
    ASSERT_TRUE(shared.has_value());
    EXPECT_EQ(*shared, "<unknown-widget/>");
    EXPECT_FALSE(SkinCompiler().prepare(resolved.snapshot).ok());
}

TEST_F(SkinResolverTest, RejectsDuplicateInstalledSkinIds) {
    makeRoot("base-one", "test.base");
    makeRoot("base-two", "test.base");
    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kLayoutResourcePath));
    writeFile(derived / "radia/xui/derived.xml", "<floater/>");

    const auto result = resolver.resolve(derived, {root});

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.back().code, "skin.id.duplicate");
}

TEST_F(SkinResolverTest, RejectsManifestResourceTraversal) {
    const std::filesystem::path selected = root / "selected";
    std::filesystem::create_directories(root / "outside");
    writeFile(selected / "manifest.json", manifest("test.selected", "\"test.base\"", R"("layouts": "../outside")"));

    const auto result = resolver.resolve(selected, {root});

    ASSERT_FALSE(result.ok());
    EXPECT_TRUE(std::any_of(result.errors.begin(), result.errors.end(),
                            [](const radia::ui::Diagnostic& diagnostic) { return diagnostic.code == "skin.manifest.path.traversal"; }));
}

TEST_F(SkinResolverTest, CapturesImportedStylesheetModulesInTheirLayer) {
    const std::filesystem::path selected = makeRoot("selected", "test.selected");
    writeFile(selected / "radia/skin.radia", "@import \"styles/panel.radia\";\nfloater { height: 220px; }");
    writeFile(selected / "radia/styles/panel.radia", "floater { width: 410px; }");

    const auto result = resolver.resolve(selected, {root});

    ASSERT_TRUE(result.ok());
    const auto& layers = result.snapshot.layers("skin.radia");
    ASSERT_EQ(layers.size(), std::size_t{1});
    EXPECT_EQ(layers.front().entrypoint, "radia/skin.radia");
    EXPECT_TRUE(layers.front().modules.contains("radia/styles/panel.radia"));
    EXPECT_TRUE(SkinCompiler().prepare(result.snapshot).ok());
}

TEST_F(SkinResolverTest, DoesNotFallBackToBaseStylesheetModules) {
    const std::filesystem::path base = makeRoot("base", "test.base");
    writeFile(base / "radia/skin.radia", "@import \"shared.radia\";");
    writeFile(base / "radia/shared.radia", "floater { width: 300px; }");

    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", R"("stylesheet": "radia/skin.radia")"));
    writeFile(derived / "radia/skin.radia", "@import \"shared.radia\";");

    const auto result = resolver.resolve(derived, {root});

    ASSERT_TRUE(result.ok());
    const auto compiled = SkinCompiler().prepare(result.snapshot);
    ASSERT_FALSE(compiled.ok());
    ASSERT_FALSE(compiled.errors.empty());
    EXPECT_EQ(compiled.errors.front().code, "stylesheet.import.missing");
    EXPECT_EQ(compiled.errors.front().source, "test.derived/radia/skin.radia");
}

TEST_F(SkinResolverTest, LoadsBundledSkinResourcesAndStableIdentities) {
    const std::filesystem::path bundled = viewerSourceRoot() / "skins/default";
    ASSERT_TRUE(std::filesystem::is_directory(bundled)) << bundled.string();

    const auto result = resolver.resolve(bundled, {bundled.parent_path()});

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.skinId, "alchemy.default");

    const auto& styleLayers = result.snapshot.layers("skin.radia");
    ASSERT_EQ(styleLayers.size(), std::size_t{1});
    EXPECT_EQ(styleLayers.front().sourceName, "alchemy.default/radia/skin.radia");
    EXPECT_EQ(styleLayers.front().entrypoint, "radia/skin.radia");
    EXPECT_TRUE(styleLayers.front().modules.contains("radia/variants.radia"));

    const auto& localizationLayers = result.snapshot.layers("localization.yaml");
    ASSERT_EQ(localizationLayers.size(), std::size_t{1});
    EXPECT_EQ(localizationLayers.front().sourceName, "alchemy.default/radia/localization.yaml");

    EXPECT_TRUE(result.snapshot.load("floater_demo.xml").has_value());
    EXPECT_TRUE(result.snapshot.load("widgets/floater.xml").has_value());
    EXPECT_TRUE(result.snapshot.load("resources/icons/search.svg").has_value());
    EXPECT_TRUE(result.snapshot.load("resources/icons/close.svg").has_value());
    EXPECT_TRUE(result.snapshot.load("resources/icons/minimize.svg").has_value());
    EXPECT_TRUE(SkinCompiler().prepare(result.snapshot).ok());
}
