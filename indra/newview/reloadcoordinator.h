/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <chrono>
#include <cstdint>
#include <memory>
#include <optional>
#include "diagnostic.h"
#include "resourceprovider.h"

namespace radia::ui { class System; } // namespace radia::ui

namespace radia::viewer::ui {
using radia::ui::DiagnosticResult;
using radia::ui::System;

class SkinSnapshotSource;
class ComponentManager;

struct SkinReloadResult : DiagnosticResult {
    bool committed = false;
    std::uint64_t generationNumber = 0;

    bool ok() const { return committed && !hasErrors(); }
};

struct SkinReloadTiming {
    std::chrono::milliseconds scanInterval{250};
    std::chrono::milliseconds settleInterval{150};
};

class SkinReloadCoordinator final {
public:
    using TimePoint = std::chrono::steady_clock::time_point;
    SkinReloadCoordinator(System& system, const SkinSnapshotSource& snapshotSource);
    ~SkinReloadCoordinator();

    SkinReloadCoordinator(const SkinReloadCoordinator&) = delete;
    SkinReloadCoordinator& operator=(const SkinReloadCoordinator&) = delete;

    void setSkinAutoReload(bool enabled);
    bool setAutoReloadTiming(SkinReloadTiming timing);
    void request();
    std::optional<SkinReloadResult> update(TimePoint now, ComponentManager& components);

private:
    class Impl;
    std::unique_ptr<Impl> mImpl;
};
} // namespace radia::viewer::ui
