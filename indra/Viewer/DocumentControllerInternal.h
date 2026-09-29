/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <memory>
#include "DocumentController.h"

namespace Viewer {
class DocumentController::PreparedMount final {
public:
    PreparedMount();
    ~PreparedMount();
    PreparedMount(PreparedMount&&) noexcept;
    PreparedMount& operator=(PreparedMount&&) noexcept;
    PreparedMount(const PreparedMount&) = delete;
    PreparedMount& operator=(const PreparedMount&) = delete;

    explicit operator bool() const;

private:
    friend class DocumentController;
    friend class ComponentManager;

    struct State;
    std::unique_ptr<State> mState;
};

struct DocumentController::PreparedMountResult : Core::DiagnosticResult {
    bool ok() const;
    PreparedMount mount;
};
} // namespace Viewer
