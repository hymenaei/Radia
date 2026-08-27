/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
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
#include "resolver.h"

namespace {
using radia::ui::SkinCompiler;
using radia::viewer::ui::SkinResolver;

constexpr char kRootResourcePaths[] =
    R"("stylesheet": "radia/skin.css","layouts": "radia/xui","localization": "radia/localization.yaml","assets": "radia/resources")";
constexpr char kLayoutResourcePath[] = R"("layouts": "radia/xui")";
constexpr char kLocalization[] = "defaultLocale: en\nlocales: {en: {strings: {title: Base}}}\n";
constexpr char kMinimalFloaterLayout[] = "<floater><head><title>Base</title></head><body/></floater>";

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
        writeFile(skin / "radia/skin.css", "floater { width: 300px; }");
        writeFile(skin / "radia/localization.yaml", kLocalization);
        writeFile(skin / "radia/xui/base.html", kMinimalFloaterLayout);
        std::filesystem::create_directories(skin / "radia/resources");
        return skin;
    }

    std::filesystem::path viewerSourceRoot() const { return std::filesystem::path(__FILE__).parent_path().parent_path(); }

    std::filesystem::path root;
    SkinResolver resolver;
};
} // namespace

TEST_F(SkinResolverTest, LayersDerivedResourcesAfterTheirBase) {
    makeRoot("base", "test.base");
    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kRootResourcePaths));
    writeFile(derived / "radia/skin.css", "floater { height: 220px; }");
    writeFile(derived / "radia/localization.yaml", "locales: {en: {strings: {title: Derived}}}\n");
    writeFile(derived / "radia/xui/derived.html", kMinimalFloaterLayout);

    constexpr char kDerivedAsset[] = R"(<svg viewBox="0 0 10 10"><circle cx="5" cy="5" r="4"/></svg>)";
    writeFile(derived / "radia/resources/icons/shared.svg", kDerivedAsset);

    const auto result = resolver.resolve(derived, {root});

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.skinId, "test.derived");
    EXPECT_TRUE(result.snapshot.load("base.html").has_value());
    EXPECT_TRUE(result.snapshot.load("derived.html").has_value());
    EXPECT_EQ(result.snapshot.layers("skin.css").size(), std::size_t{2});
    EXPECT_EQ(result.snapshot.layers("localization.yaml").size(), std::size_t{2});

    const auto asset = result.snapshot.load("resources/icons/shared.svg");
    ASSERT_TRUE(asset.has_value());
    EXPECT_EQ(*asset, kDerivedAsset);
    const auto compiled = SkinCompiler().prepare(result.snapshot);
    if (!compiled.ok())
        for (const auto& error : compiled.errors) ADD_FAILURE() << error.formatted();
    EXPECT_TRUE(compiled.ok());
}

TEST_F(SkinResolverTest, DoesNotMergeResourcesFromUnrelatedSkins) {
    makeRoot("unrelated", "test.unrelated");
    writeFile(root / "unrelated/radia/xui/unrelated.html", kMinimalFloaterLayout);
    const std::filesystem::path selected = makeRoot("selected", "test.selected");

    const auto result = resolver.resolve(selected, {root});

    ASSERT_TRUE(result.ok());
    EXPECT_TRUE(result.snapshot.load("base.html").has_value());
    EXPECT_FALSE(result.snapshot.load("unrelated.html").has_value());
    EXPECT_EQ(result.snapshot.layers("skin.css").size(), std::size_t{1});
}

TEST_F(SkinResolverTest, RejectsBaseSkinCycles) {
    const std::filesystem::path first = root / "first";
    const std::filesystem::path second = root / "second";
    writeFile(first / "manifest.json", manifest("test.first", "\"test.second\"", kLayoutResourcePath));
    writeFile(first / "radia/xui/first.html", kMinimalFloaterLayout);
    writeFile(second / "manifest.json", manifest("test.second", "\"test.first\"", kLayoutResourcePath));
    writeFile(second / "radia/xui/second.html", kMinimalFloaterLayout);

    const auto result = resolver.resolve(first, {root});

    ASSERT_FALSE(result.ok());
    ASSERT_FALSE(result.errors.empty());
    EXPECT_EQ(result.errors.back().code, "skin.base.cycle");
}

TEST_F(SkinResolverTest, RejectsMalformedDerivedLayoutOverrides) {
    const std::filesystem::path base = makeRoot("base", "test.base");
    writeFile(base / "radia/xui/shared.html", kMinimalFloaterLayout);
    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kLayoutResourcePath));
    writeFile(derived / "radia/xui/shared.html", "<unknown-element/>");

    const auto resolved = resolver.resolve(derived, {root});

    ASSERT_TRUE(resolved.ok());
    const auto shared = resolved.snapshot.load("shared.html");
    ASSERT_TRUE(shared.has_value());
    EXPECT_EQ(*shared, "<unknown-element/>");
    EXPECT_FALSE(SkinCompiler().prepare(resolved.snapshot).ok());
}

TEST_F(SkinResolverTest, RejectsDuplicateInstalledSkinIds) {
    makeRoot("base-one", "test.base");
    makeRoot("base-two", "test.base");
    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", kLayoutResourcePath));
    writeFile(derived / "radia/xui/derived.html", kMinimalFloaterLayout);

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
    writeFile(selected / "radia/skin.css", "@import \"styles/panel.css\";\nfloater { height: 220px; }");
    writeFile(selected / "radia/styles/panel.css", "floater { width: 410px; }");

    const auto result = resolver.resolve(selected, {root});

    ASSERT_TRUE(result.ok());
    const auto& layers = result.snapshot.layers("skin.css");
    ASSERT_EQ(layers.size(), std::size_t{1});
    EXPECT_EQ(layers.front().entrypoint, "radia/skin.css");
    EXPECT_TRUE(layers.front().modules.contains("radia/styles/panel.css"));
    const auto compiled = SkinCompiler().prepare(result.snapshot);
    ASSERT_TRUE(compiled.ok()) << (compiled.errors.empty() ? "unknown skin preparation error" : compiled.errors.front().formatted());
}

TEST_F(SkinResolverTest, DoesNotFallBackToBaseStylesheetModules) {
    const std::filesystem::path base = makeRoot("base", "test.base");
    writeFile(base / "radia/skin.css", "@import \"shared.css\";");
    writeFile(base / "radia/shared.css", "floater { width: 300px; }");

    const std::filesystem::path derived = root / "derived";
    writeFile(derived / "manifest.json", manifest("test.derived", "\"test.base\"", R"("stylesheet": "radia/skin.css")"));
    writeFile(derived / "radia/skin.css", "@import \"shared.css\";");

    const auto result = resolver.resolve(derived, {root});

    ASSERT_TRUE(result.ok());
    const auto compiled = SkinCompiler().prepare(result.snapshot);
    ASSERT_FALSE(compiled.ok());
    ASSERT_FALSE(compiled.errors.empty());
    EXPECT_EQ(compiled.errors.front().code, "stylesheet.import.missing");
    EXPECT_EQ(compiled.errors.front().source, "test.derived/radia/skin.css");
}

TEST_F(SkinResolverTest, LoadsBundledSkinResourcesAndStableIdentities) {
    const std::filesystem::path bundled = viewerSourceRoot() / "skins/default";
    ASSERT_TRUE(std::filesystem::is_directory(bundled)) << bundled.string();

    const auto result = resolver.resolve(bundled, {bundled.parent_path()});

    ASSERT_TRUE(result.ok());
    EXPECT_EQ(result.skinId, "alchemy.default");

    const auto& styleLayers = result.snapshot.layers("skin.css");
    ASSERT_EQ(styleLayers.size(), std::size_t{1});
    EXPECT_EQ(styleLayers.front().sourceName, "alchemy.default/radia/skin.css");
    EXPECT_EQ(styleLayers.front().entrypoint, "radia/skin.css");
    EXPECT_TRUE(styleLayers.front().modules.contains("radia/variants.css"));

    const auto& localizationLayers = result.snapshot.layers("localization.yaml");
    ASSERT_EQ(localizationLayers.size(), std::size_t{1});
    EXPECT_EQ(localizationLayers.front().sourceName, "alchemy.default/radia/localization.yaml");

    EXPECT_TRUE(result.snapshot.load("floater_demo.html").has_value());
    EXPECT_TRUE(result.snapshot.load("elements/close.html").has_value());
    EXPECT_TRUE(result.snapshot.load("elements/minimize.html").has_value());
    EXPECT_TRUE(result.snapshot.load("resources/icons/search.svg").has_value());
    EXPECT_TRUE(result.snapshot.load("resources/icons/close.svg").has_value());
    EXPECT_TRUE(result.snapshot.load("resources/icons/minimize.svg").has_value());
    const auto compiled = SkinCompiler().prepare(result.snapshot);
    EXPECT_TRUE(compiled.ok()) << (compiled.errors.empty() ? "unknown skin preparation error" : compiled.errors.front().formatted());
}
