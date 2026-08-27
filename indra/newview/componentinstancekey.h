/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <optional>
#include <string>
#include <string_view>
#include <tuple>
#include <utility>

namespace radia::viewer::ui {
struct ComponentInstanceKey {
    std::string definitionId;
    std::string instanceKey;

    bool operator==(const ComponentInstanceKey&) const = default;
    bool operator<(const ComponentInstanceKey& other) const {
        return std::tie(definitionId, instanceKey) < std::tie(other.definitionId, other.instanceKey);
    }

    bool valid() const { return !definitionId.empty() && validPart(definitionId) && validInstanceKey(instanceKey); }

    std::string persistenceKey() const { return instanceKey.empty() ? definitionId : instanceKey + "@" + definitionId; }

    static std::optional<ComponentInstanceKey> fromPersistenceKey(std::string_view value) {
        if (value.empty()) return std::nullopt;

        const std::size_t separator = value.find('@');
        if (separator == std::string_view::npos) {
            ComponentInstanceKey result{std::string(value), {}};
            return result.valid() ? std::optional<ComponentInstanceKey>(std::move(result)) : std::nullopt;
        }
        if (separator == 0 || separator + 1 >= value.size() || value.find('@', separator + 1) != std::string_view::npos) return std::nullopt;

        ComponentInstanceKey result{std::string(value.substr(separator + 1)), std::string(value.substr(0, separator))};
        return result.valid() ? std::optional<ComponentInstanceKey>(std::move(result)) : std::nullopt;
    }

private:
    static bool validPart(std::string_view value) {
        return !value.empty() && value.find('/') == std::string_view::npos && value.find('@') == std::string_view::npos;
    }

    static bool validInstanceKey(std::string_view value) { return value.empty() || validPart(value); }
};
} // namespace radia::viewer::ui
