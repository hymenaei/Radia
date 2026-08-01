/**
 * @file reloadcoordinator.cpp
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

#include "llviewerprecompiledheaders.h"
#include "skin/reloadcoordinator.h"
#include <utility>
#include "floaterdocumentmanager.h"
#include "skin/compiler.h"
#include "skin/generation.h"
#include "skin/resolver.h"
#include "system.h"

namespace rdui::viewer {
namespace {
constexpr std::chrono::milliseconds SCAN_INTERVAL{250};
constexpr std::chrono::milliseconds SETTLE_INTERVAL{150};

bool equalSnapshots(const ResourceSnapshot& left, const ResourceSnapshot& right) {
    return left.resources() == right.resources() && left.layeredResources() == right.layeredResources();
}
} // namespace

class SkinReloadCoordinator::Impl {
public:
    Impl(System& system, const SkinSnapshotSource& snapshots) : mSystem(system), mSnapshots(snapshots) {}

    void setAuthoringEnabled(bool enabled) {
        if (mAuthoringEnabled == enabled) return;
        mAuthoringEnabled = enabled;
        mObserved.reset();
        mAcknowledged.reset();
        mNextScan = {};
        mLastChange = {};
        mSettling = false;
        mRetryAfterAnyChange = false;
    }

    void request() {
        mRequested = true;
        mRequestedSnapshot.reset();
    }

    std::optional<SkinReloadResult> update(TimePoint now, FloaterDocumentManager& documents) {
        if (std::optional<SkinSnapshotResult> settled = poll(now)) {
            mRequestedSnapshot = std::move(settled);
            mRequested = true;
        }
        if (!mRequested) return std::nullopt;

        mRequested = false;
        SkinSnapshotResult captured = mRequestedSnapshot ? std::move(*mRequestedSnapshot) : mSnapshots.capture();
        mRequestedSnapshot.reset();
        acknowledge(captured.snapshot, now);
        SkinReloadResult result = reload(std::move(captured), documents);
        mRetryAfterAnyChange = !result.ok();
        return result;
    }

private:
    std::optional<SkinSnapshotResult> poll(TimePoint now) {
        if (!mAuthoringEnabled) return std::nullopt;
        if (mNextScan != TimePoint() && now < mNextScan) return std::nullopt;
        mNextScan = now + SCAN_INTERVAL;

        SkinSnapshotResult captured = mSnapshots.capture();
        ResourceSnapshot& current = captured.snapshot;
        if (!mObserved) {
            mObserved = current;
            mAcknowledged = current;
            return std::nullopt;
        }
        if (!mSettling) {
            const bool unchanged =
                mAcknowledged && (mRetryAfterAnyChange ? equalSnapshots(current, *mAcknowledged) : mSystem.sameReloadInputs(current, *mAcknowledged));
            if (unchanged) {
                mObserved = current;
                mAcknowledged = current;
                return std::nullopt;
            }
            mObserved = std::move(current);
            mLastChange = now;
            mSettling = true;
            return std::nullopt;
        }
        if (!equalSnapshots(current, *mObserved)) {
            mObserved = std::move(current);
            mLastChange = now;
            return std::nullopt;
        }
        if (mLastChange == TimePoint() || now - mLastChange < SETTLE_INTERVAL) return std::nullopt;

        mAcknowledged = *mObserved;
        captured.snapshot = *mObserved;
        mSettling = false;
        return captured;
    }

    void acknowledge(const ResourceSnapshot& snapshot, TimePoint now) {
        if (!mAuthoringEnabled) return;
        mObserved = snapshot;
        mAcknowledged = snapshot;
        mLastChange = now;
        mSettling = false;
    }

    SkinReloadResult reload(SkinSnapshotResult captured, FloaterDocumentManager& documents) {
        SkinReloadResult result;
        const bool capture_ok = captured.ok();
        result.append(std::move(captured));
        if (!capture_ok) {
            result.generation = mSystem.generation();
            return result;
        }

        SkinGenerationPrepareResult prepared = mCompiler.prepare(std::move(captured.snapshot));
        const bool generation_ok = prepared.ok();
        result.append(std::move(prepared));
        if (!generation_ok) {
            result.generation = mSystem.generation();
            return result;
        }

        FloaterDocumentManager::ReplacementResult replacement = documents.prepareReplacement(*prepared.generation, mSystem.activeLocale());
        const bool replacement_ok = replacement.ok();
        result.append(std::move(replacement));
        if (!replacement_ok) {
            result.generation = mSystem.generation();
            return result;
        }

        mSystem.publish(std::move(prepared.generation), [&replacement] { replacement.replacement.commit(); });
        result.committed = !replacement.replacement;
        result.generation = mSystem.generation();
        return result;
    }

    System& mSystem;
    const SkinSnapshotSource& mSnapshots;
    SkinCompiler mCompiler;
    bool mAuthoringEnabled = false;
    bool mRequested = false;
    std::optional<SkinSnapshotResult> mRequestedSnapshot;
    std::optional<ResourceSnapshot> mObserved;
    std::optional<ResourceSnapshot> mAcknowledged;
    TimePoint mNextScan;
    TimePoint mLastChange;
    bool mSettling = false;
    bool mRetryAfterAnyChange = false;
};

SkinReloadCoordinator::SkinReloadCoordinator(System& system, const SkinSnapshotSource& snapshots)
    : mImpl(std::make_unique<Impl>(system, snapshots)) {}

SkinReloadCoordinator::~SkinReloadCoordinator() = default;

void SkinReloadCoordinator::setAuthoringEnabled(bool enabled) {
    mImpl->setAuthoringEnabled(enabled);
}

void SkinReloadCoordinator::request() {
    mImpl->request();
}

std::optional<SkinReloadResult> SkinReloadCoordinator::update(TimePoint now, FloaterDocumentManager& documents) {
    return mImpl->update(now, documents);
}
} // namespace rdui::viewer
