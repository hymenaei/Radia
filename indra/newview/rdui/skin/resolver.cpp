/**
 * @file resolver.cpp
 * @brief Resolves selected and installed skin roots into validated UI resource snapshots.
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

#include "llviewerprecompiledheaders.h"
#include "skin/resolver.h"
#include <algorithm>
#include <fstream>
#include <initializer_list>
#include <iterator>
#include <map>
#include <optional>
#include <unordered_set>
#include "llsd.h"
#include "llsdjson.h"

namespace rdui::viewer {
namespace {
struct ManifestResources {
    std::optional<std::filesystem::path> stylesheet;
    std::optional<std::filesystem::path> layouts;
    std::optional<std::filesystem::path> localization;
    std::optional<std::filesystem::path> assets;
};

struct SkinManifest {
    std::string id;
    std::optional<std::string> base;
    std::filesystem::path root;
    ManifestResources resources;
};

struct ManifestResult : DiagnosticResult {
    std::optional<SkinManifest> manifest;
    bool ok() const { return !hasErrors() && manifest.has_value(); }
};

std::optional<std::string> readFile(const std::filesystem::path& filename) {
    std::ifstream input(filename, std::ios::binary);
    if (!input.is_open()) return std::nullopt;
    return std::string(std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>());
}

bool validSkinId(const std::string& id) {
    if (id.empty()) return false;
    std::size_t segmentStart = 0;
    while (segmentStart < id.size()) {
        const std::size_t dot = id.find('.', segmentStart);
        const std::size_t segmentEnd = dot == std::string::npos ? id.size() : dot;
        if (segmentEnd == segmentStart || id[segmentStart] == '-' || id[segmentEnd - 1] == '-') return false;
        for (std::size_t index = segmentStart; index < segmentEnd; ++index) {
            const unsigned char character = static_cast<unsigned char>(id[index]);
            if (!((character >= 'a' && character <= 'z') || (character >= '0' && character <= '9') || character == '-')) return false;
        }
        if (dot == std::string::npos) return true;
        segmentStart = dot + 1;
    }
    return false;
}

bool allowedKey(const std::string& key, std::initializer_list<const char*> allowed) {
    return std::any_of(allowed.begin(), allowed.end(), [&key](const char* candidate) { return key == candidate; });
}

void validateKeys(const LLSD& object, std::initializer_list<const char*> allowed, DiagnosticResult& result, const std::string& source,
                  const std::string& context) {
    if (!object.isMap()) return;
    for (auto entry = object.beginMap(); entry != object.endMap(); ++entry)
        if (!allowedKey(entry->first, allowed))
            result.error("skin.manifest.key_unknown", "Unknown " + context + " key: " + entry->first + ".", source);
}

bool requireString(const LLSD& object, const char* key, std::string& value, DiagnosticResult& result, const std::string& source) {
    if (!object.has(key)) {
        result.error("skin.manifest.field_missing", "Missing required manifest field: " + std::string(key) + ".", source);
        return false;
    }
    if (!object[key].isString() || object[key].asString().empty()) {
        result.error("skin.manifest.field_invalid", "Manifest field '" + std::string(key) + "' must be a non-empty string.", source);
        return false;
    }
    value = object[key].asString();
    return true;
}

bool inside(const std::filesystem::path& path, const std::filesystem::path& root) {
    auto pathPart = path.begin();
    auto rootPart = root.begin();
    for (; rootPart != root.end(); ++rootPart, ++pathPart)
        if (pathPart == path.end() || *pathPart != *rootPart) return false;
    return true;
}

std::optional<std::filesystem::path> resourcePath(const LLSD& radia, const char* key, const std::filesystem::path& skinRoot, bool directory,
                                                  DiagnosticResult& result, const std::string& source) {
    if (!radia.has(key)) return std::nullopt;
    if (!radia[key].isString() || radia[key].asString().empty()) {
        result.error("skin.manifest.resource.invalid", "Radia resource '" + std::string(key) + "' must be a non-empty relative path.", source);
        return std::nullopt;
    }

    const std::string value = radia[key].asString();
    const std::filesystem::path relative(value);
    if (relative.is_absolute() || relative.has_root_name() || value.find("://") != std::string::npos) {
        result.error("skin.manifest.path.invalid", "Radia resource path must be Skin-relative: " + value + ".", source);
        return std::nullopt;
    }
    for (const auto& part : relative) {
        if (part == "..") {
            result.error("skin.manifest.path.traversal", "Radia resource path escapes its Skin: " + value + ".", source);
            return std::nullopt;
        }
    }

    std::error_code error;
    const std::filesystem::path canonicalRoot = std::filesystem::canonical(skinRoot, error);
    if (error) {
        result.error("skin.manifest.root.invalid", "Could not canonicalize Skin root.", source);
        return std::nullopt;
    }
    const std::filesystem::path resolved = std::filesystem::canonical(skinRoot / relative, error);
    if (error || !inside(resolved, canonicalRoot)) {
        result.error("skin.manifest.resource.missing", "Radia resource is missing or outside its Skin: " + value + ".", source);
        return std::nullopt;
    }
    const bool correctType = directory ? std::filesystem::is_directory(resolved, error) : std::filesystem::is_regular_file(resolved, error);
    if (error || !correctType) {
        result.error("skin.manifest.resource.type_invalid", "Radia resource has the wrong filesystem type: " + value + ".", source);
        return std::nullopt;
    }
    return resolved;
}

ManifestResult parseManifest(const std::filesystem::path& root) {
    ManifestResult result;
    std::error_code rootError;
    const std::filesystem::path canonicalRoot = std::filesystem::canonical(root, rootError);
    if (rootError || !std::filesystem::is_directory(canonicalRoot, rootError)) {
        result.error("skin.manifest.root.invalid", "Skin root is missing or invalid.", root.generic_string());
        return result;
    }
    const std::filesystem::path filename = canonicalRoot / "manifest.json";
    const std::string source = filename.generic_string();
    const std::optional<std::string> json = readFile(filename);
    if (!json) {
        result.error("skin.manifest.missing", "Skin has no manifest.json.", source);
        return result;
    }

    LLSD document;
    std::string parseError;
    if (!LlsdFromJsonString(*json, document, &parseError) || !document.isMap()) {
        result.error("skin.manifest.json_invalid", "Invalid Skin manifest JSON: " + parseError + ".", source);
        return result;
    }
    validateKeys(document, {"id", "name", "author", "url", "notes", "preview", "base", "radia"}, result, source, "manifest");

    SkinManifest manifest;
    manifest.root = canonicalRoot;
    std::string ignored;
    requireString(document, "id", manifest.id, result, source);
    requireString(document, "name", ignored, result, source);
    requireString(document, "author", ignored, result, source);
    if (!manifest.id.empty() && !validSkinId(manifest.id))
        result.error("skin.manifest.id_invalid", "Invalid stable Skin ID: " + manifest.id + ".", source);

    if (!document.has("base")) result.error("skin.manifest.field_missing", "Missing required manifest field: base.", source);
    else if (document["base"].isUndefined()) manifest.base.reset();
    else if (!document["base"].isString() || !validSkinId(document["base"].asString()))
        result.error("skin.manifest.base_invalid", "Manifest base must be null or a valid stable Skin ID.", source);
    else manifest.base = document["base"].asString();

    if (!document.has("radia")) result.error("skin.manifest.field_missing", "Missing required manifest field: radia.", source);
    else if (!document["radia"].isMap()) result.error("skin.manifest.radia_invalid", "Manifest radia field must be an object.", source);
    else {
        const LLSD& radia = document["radia"];
        validateKeys(radia, {"stylesheet", "layouts", "localization", "assets"}, result, source, "radia");
        const bool rootSkin = document.has("base") && document["base"].isUndefined();
        if (rootSkin) {
            for (const char* key : {"stylesheet", "layouts", "localization", "assets"})
                if (!radia.has(key))
                    result.error("skin.manifest.resource.required", "Root Skin must declare Radia resource: " + std::string(key) + ".", source);
        } else if (radia.size() == 0) result.error("skin.manifest.radia_empty", "Derived Skin must declare at least one Radia resource.", source);

        manifest.resources.stylesheet = resourcePath(radia, "stylesheet", canonicalRoot, false, result, source);
        manifest.resources.layouts = resourcePath(radia, "layouts", canonicalRoot, true, result, source);
        manifest.resources.localization = resourcePath(radia, "localization", canonicalRoot, false, result, source);
        manifest.resources.assets = resourcePath(radia, "assets", canonicalRoot, true, result, source);
    }

    for (const char* optional : {"url", "notes", "preview"})
        if (document.has(optional) && !document[optional].isString())
            result.error("skin.manifest.field_invalid", "Optional manifest field '" + std::string(optional) + "' must be a string.", source);
    if (!result.hasErrors()) result.manifest = std::move(manifest);
    return result;
}

std::string sourceName(const SkinManifest& manifest, const std::filesystem::path& path) {
    std::error_code error;
    const std::filesystem::path relative = std::filesystem::relative(path, manifest.root, error);
    return manifest.id + "/" + (error ? path.filename().generic_string() : relative.generic_string());
}

void addLayer(const SkinManifest& manifest, const std::filesystem::path& path, std::vector<ResourceLayer>& layers, SkinSnapshotResult& result) {
    const std::optional<std::string> source = readFile(path);
    if (!source) {
        result.error("skin.resource.read_failed", "Could not read declared Skin resource.", sourceName(manifest, path));
        return;
    }
    layers.push_back({sourceName(manifest, path), *source});
}

void addStyleLayer(const SkinManifest& manifest, const std::filesystem::path& entrypoint, std::vector<ResourceLayer>& layers,
                   SkinSnapshotResult& result) {
    const std::optional<std::string> source = readFile(entrypoint);
    if (!source) {
        result.error("skin.resource.read_failed", "Could not read declared Skin resource.", sourceName(manifest, entrypoint));
        return;
    }

    std::error_code error;
    const std::filesystem::path relativeEntrypoint = std::filesystem::relative(entrypoint, manifest.root, error);
    if (error) {
        result.error("skin.resource.path_invalid", "Could not identify the stylesheet entrypoint inside its Skin.", sourceName(manifest, entrypoint));
        return;
    }

    ResourceLayer layer{sourceName(manifest, entrypoint), *source};
    layer.entrypoint = relativeEntrypoint.generic_string();
    for (std::filesystem::recursive_directory_iterator iterator(manifest.root, error), end; iterator != end && !error; iterator.increment(error)) {
        std::error_code fileError;
        if (!iterator->is_regular_file(fileError) || fileError || iterator->path().extension() != ".radia") continue;
        const std::filesystem::path canonicalFile = std::filesystem::canonical(iterator->path(), fileError);
        if (fileError || !inside(canonicalFile, manifest.root)) continue;
        const std::filesystem::path relative = std::filesystem::relative(canonicalFile, manifest.root, fileError);
        if (fileError) continue;
        if (relative == relativeEntrypoint) continue;
        const std::optional<std::string> module = readFile(canonicalFile);
        if (module) layer.modules.insert_or_assign(relative.generic_string(), *module);
    }
    if (error) {
        result.error("skin.resource.discovery_failed", "Could not enumerate stylesheet modules inside the Skin.", manifest.root.generic_string());
        return;
    }
    layers.push_back(std::move(layer));
}

void overlayDirectory(const SkinManifest& manifest, const std::filesystem::path& root, const std::string& logicalPrefix, ResourceSnapshot& snapshot,
                      SkinSnapshotResult& result) {
    std::error_code error;
    for (std::filesystem::recursive_directory_iterator iterator(root, error), end; iterator != end && !error; iterator.increment(error)) {
        if (!iterator->is_regular_file(error) || error) continue;
        const std::filesystem::path canonicalFile = std::filesystem::canonical(iterator->path(), error);
        const std::filesystem::path canonicalSkin = std::filesystem::canonical(manifest.root, error);
        if (error || !inside(canonicalFile, canonicalSkin)) {
            result.error("skin.resource.path_invalid", "Discovered resource escapes its Skin.", iterator->path().generic_string());
            error.clear();
            continue;
        }
        const std::filesystem::path relative = std::filesystem::relative(iterator->path(), root, error);
        if (error) break;
        const std::optional<std::string> source = readFile(iterator->path());
        if (!source) {
            result.error("skin.resource.read_failed", "Could not read Skin resource.", sourceName(manifest, iterator->path()));
            continue;
        }
        snapshot.add(logicalPrefix + relative.generic_string(), *source);
    }
    if (error) result.error("skin.resource.discovery_failed", "Could not enumerate declared Skin resource directory.", root.generic_string());
}
} // namespace

SkinSnapshotResult SkinResolver::resolve(const std::filesystem::path& selectedRoot,
                                         const std::vector<std::filesystem::path>& installedRoots) const {
    SkinSnapshotResult result;
    ManifestResult selectedResult = parseManifest(selectedRoot);
    if (!selectedResult.ok()) {
        result.append(std::move(selectedResult));
        return result;
    }

    std::map<std::string, std::vector<SkinManifest>> installed;
    SkinManifest selected = std::move(*selectedResult.manifest);
    result.skinId = selected.id;
    installed[selected.id].push_back(selected);

    std::error_code error;
    const std::filesystem::path canonicalSelected = std::filesystem::canonical(selectedRoot, error);
    for (const std::filesystem::path& catalogRoot : installedRoots) {
        error.clear();
        if (!std::filesystem::is_directory(catalogRoot, error) || error) continue;
        const std::filesystem::path canonicalCatalog = std::filesystem::canonical(catalogRoot, error);
        if (error) continue;
        for (std::filesystem::directory_iterator iterator(catalogRoot, error), end; iterator != end && !error; iterator.increment(error)) {
            if (!iterator->is_directory(error) || error) continue;
            const std::filesystem::path candidateRoot = std::filesystem::canonical(iterator->path(), error);
            if (error) break;
            if (!inside(candidateRoot, canonicalCatalog)) continue;
            if (candidateRoot == canonicalSelected) continue;
            ManifestResult candidate = parseManifest(candidateRoot);
            if (candidate.ok()) installed[candidate.manifest->id].push_back(std::move(*candidate.manifest));
        }
    }

    std::vector<const SkinManifest*> chain;
    const SkinManifest* current = &installed[selected.id].front();
    std::unordered_set<std::string> visited;
    while (current) {
        if (!visited.insert(current->id).second) {
            result.error("skin.base.cycle", "Base Skin cycle contains: " + current->id + ".", current->root.generic_string());
            return result;
        }
        chain.push_back(current);
        if (!current->base) break;
        const auto base = installed.find(*current->base);
        if (base == installed.end()) {
            result.error("skin.base.missing", "Base Skin is not installed: " + *current->base + ".", current->root.generic_string());
            return result;
        }
        if (base->second.size() != 1) {
            result.error("skin.id.duplicate", "Multiple installed Skins use ID: " + *current->base + ".", current->root.generic_string());
            return result;
        }
        current = &base->second.front();
    }
    if (installed[selected.id].size() != 1) {
        result.error("skin.id.duplicate", "Multiple installed Skins use selected ID: " + selected.id + ".", selected.root.generic_string());
        return result;
    }
    std::reverse(chain.begin(), chain.end());

    std::vector<ResourceLayer> styleLayers;
    std::vector<ResourceLayer> localizationLayers;
    for (const SkinManifest* manifest : chain) {
        if (manifest->resources.stylesheet) addStyleLayer(*manifest, *manifest->resources.stylesheet, styleLayers, result);
        if (manifest->resources.localization) addLayer(*manifest, *manifest->resources.localization, localizationLayers, result);
        if (manifest->resources.layouts) overlayDirectory(*manifest, *manifest->resources.layouts, "", result.snapshot, result);
        if (manifest->resources.assets) overlayDirectory(*manifest, *manifest->resources.assets, "resources/", result.snapshot, result);
    }

    if (!styleLayers.empty()) {
        result.snapshot.add("skin.radia", styleLayers.back().source);
        result.snapshot.setLayers("skin.radia", std::move(styleLayers));
    }
    if (!localizationLayers.empty()) {
        result.snapshot.add("localization.yaml", localizationLayers.back().source);
        result.snapshot.setLayers("localization.yaml", std::move(localizationLayers));
    }
    return result;
}
} // namespace rdui::viewer
