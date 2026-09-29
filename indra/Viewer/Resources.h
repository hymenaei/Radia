/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include "SkinSnapshot.h"

namespace Viewer {
class SkinResources final : public SkinSnapshotSource {
public:
    SkinSnapshotResult capture() const override;
    SkinSnapshotResult captureBundledDefault() const;
    bool selectedIsBundledDefault() const;
};
} // namespace Viewer
