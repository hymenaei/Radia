/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "reloadcoordinator.h"
#include <utility>
#include "componentmanager.h"
#include "resolver.h"
#include "skinpreparation.h"
#include "system.h"

namespace radia::viewer::ui {
using radia::ui::ResourceSnapshot;
using radia::ui::SkinGenerationPrepareResult;
using radia::ui::System;

namespace {
bool equalSnapshots(const ResourceSnapshot& left, const ResourceSnapshot& right) {
    return left.resources() == right.resources() && left.layeredResources() == right.layeredResources();
}
} // namespace

class SkinReloadCoordinator::Impl {
public:
    Impl(System& system, const SkinSnapshotSource& snapshotSource) : mSystem(system), mSnapshotSource(snapshotSource) {}

    void setSkinAutoReload(bool enabled) {
        if (mSkinAutoReload == enabled) return;
        mSkinAutoReload = enabled;
        mObservedSnapshot.reset();
        mAcknowledgedSnapshot.reset();
        mNextScan = {};
        mLastChange = {};
        mSettling = false;
        mRetryAfterAnyChange = false;
        if (enabled) mRequested = true;
    }

    bool setAutoReloadTiming(SkinReloadTiming timing) {
        if (timing.scanInterval.count() <= 0 || timing.settleInterval.count() <= 0) return false;
        if (mTiming.scanInterval == timing.scanInterval && mTiming.settleInterval == timing.settleInterval) return true;
        mTiming = timing;
        mNextScan = {};
        return true;
    }

    void request() {
        mRequested = true;
        mRequestedSnapshot.reset();
    }

    std::optional<SkinReloadResult> update(TimePoint now, ComponentManager& components) {
        if (!mRequested) {
            if (std::optional<SkinSnapshotResult> settled = poll(now)) {
                mRequestedSnapshot = std::move(settled);
                mRequested = true;
            }
        }
        if (!mRequested) return std::nullopt;

        mRequested = false;
        SkinSnapshotResult captured = mRequestedSnapshot ? std::move(*mRequestedSnapshot) : mSnapshotSource.capture();
        mRequestedSnapshot.reset();
        acknowledge(captured.snapshot, now);
        SkinReloadResult result = reload(std::move(captured), components);
        mRetryAfterAnyChange = !result.ok();
        return result;
    }

private:
    std::optional<SkinSnapshotResult> poll(TimePoint now) {
        if (!mSkinAutoReload) return std::nullopt;
        if (mNextScan != TimePoint() && now < mNextScan) return std::nullopt;
        mNextScan = now + mTiming.scanInterval;

        SkinSnapshotResult captured = mSnapshotSource.capture();
        ResourceSnapshot& current = captured.snapshot;
        if (!mObservedSnapshot) {
            mObservedSnapshot = current;
            mAcknowledgedSnapshot = current;
            return std::nullopt;
        }
        if (!mSettling) {
            const bool unchanged = mAcknowledgedSnapshot
                && (mRetryAfterAnyChange ? equalSnapshots(current, *mAcknowledgedSnapshot)
                                         : !mSystem.hasRelevantStyleChange(current, *mAcknowledgedSnapshot));
            if (unchanged) {
                mObservedSnapshot = current;
                mAcknowledgedSnapshot = current;
                return std::nullopt;
            }
            mObservedSnapshot = std::move(current);
            mLastChange = now;
            mSettling = true;
            return std::nullopt;
        }
        if (!equalSnapshots(current, *mObservedSnapshot)) {
            mObservedSnapshot = std::move(current);
            mLastChange = now;
            return std::nullopt;
        }
        if (mLastChange == TimePoint() || now - mLastChange < mTiming.settleInterval) return std::nullopt;

        mAcknowledgedSnapshot = *mObservedSnapshot;
        captured.snapshot = *mObservedSnapshot;
        mSettling = false;
        return captured;
    }

    void acknowledge(const ResourceSnapshot& snapshot, TimePoint now) {
        if (!mSkinAutoReload) return;
        mObservedSnapshot = snapshot;
        mAcknowledgedSnapshot = snapshot;
        mLastChange = now;
        mSettling = false;
        if (mNextScan == TimePoint() || now >= mNextScan) mNextScan = now + mTiming.scanInterval;
    }

    SkinReloadResult reload(SkinSnapshotResult captured, ComponentManager& components) {
        SkinReloadResult result;
        SkinGenerationPrepareResult prepared = prepareSkinGeneration(std::move(captured));
        const bool generationOk = prepared.ok();
        auto generation = std::move(prepared.generation);
        result.append(std::move(prepared));
        if (!generationOk) {
            result.generationNumber = mSystem.generation();
            return result;
        }

        ComponentManager::ReplacementResult replacement = components.prepareReplacement(generation, mSystem.activeLocale());
        const bool replacementOk = replacement.ok();
        result.append(std::move(replacement));
        if (!replacementOk) {
            result.generationNumber = mSystem.generation();
            return result;
        }

        const bool committed = mSystem.publish(std::move(generation), replacement.replacement);
        result.append(replacement.replacement.takeDiagnostics());
        if (!committed) {
            if (result.errors.empty()) result.error("floater.host.replace_failed", "The Floater host rejected the prepared replacements.");
            result.generationNumber = mSystem.generation();
            return result;
        }
        result.committed = true;
        result.generationNumber = mSystem.generation();
        return result;
    }

    System& mSystem;
    const SkinSnapshotSource& mSnapshotSource;
    SkinReloadTiming mTiming;
    bool mSkinAutoReload = false;
    bool mRequested = false;
    std::optional<SkinSnapshotResult> mRequestedSnapshot;
    std::optional<ResourceSnapshot> mObservedSnapshot;
    std::optional<ResourceSnapshot> mAcknowledgedSnapshot;
    TimePoint mNextScan;
    TimePoint mLastChange;
    bool mSettling = false;
    bool mRetryAfterAnyChange = false;
};

SkinReloadCoordinator::SkinReloadCoordinator(System& system, const SkinSnapshotSource& snapshotSource)
    : mImpl(std::make_unique<Impl>(system, snapshotSource)) {}

SkinReloadCoordinator::~SkinReloadCoordinator() = default;

void SkinReloadCoordinator::setSkinAutoReload(bool enabled) {
    mImpl->setSkinAutoReload(enabled);
}

bool SkinReloadCoordinator::setAutoReloadTiming(SkinReloadTiming timing) {
    return mImpl->setAutoReloadTiming(timing);
}

void SkinReloadCoordinator::request() {
    mImpl->request();
}

std::optional<SkinReloadResult> SkinReloadCoordinator::update(TimePoint now, ComponentManager& components) {
    return mImpl->update(now, components);
}
} // namespace radia::viewer::ui
