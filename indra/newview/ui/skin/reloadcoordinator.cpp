/**
 * @file reloadcoordinator.cpp
 * @brief Coordinates debounced skin reloads and commits complete UI skin generations.
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
#include <set>
#include <utility>
#include "componentmanager.h"
#include "skin/compiler.h"
#include "skin/generation.h"
#include "skin/resolver.h"
#include "system.h"

namespace radia::viewer::ui {
using namespace ::radia::ui;
namespace {
bool equalSnapshots(const ResourceSnapshot& left, const ResourceSnapshot& right) {
    return left.resources() == right.resources() && left.layeredResources() == right.layeredResources();
}

bool equalReloadInputs(const ResourceSnapshot& left, const ResourceSnapshot& right, const ResourceDependencyMap& dependencies) {
    if (left.resources() != right.resources()) return false;
    const auto& leftResources = left.layeredResources();
    const auto& rightResources = right.layeredResources();
    if (leftResources.size() != rightResources.size()) return false;

    std::set<std::string> relevantSources;
    for (const auto& [source, imports] : dependencies) {
        relevantSources.insert(source);
        relevantSources.insert(imports.begin(), imports.end());
    }

    for (const auto& [resourceId, leftLayers] : leftResources) {
        const auto rightResource = rightResources.find(resourceId);
        if (rightResource == rightResources.end()) return false;
        const auto& rightLayers = rightResource->second;
        if (resourceId != "skin.radia") {
            if (leftLayers != rightLayers) return false;
            continue;
        }
        if (leftLayers.size() != rightLayers.size()) return false;
        for (std::size_t index = 0; index < leftLayers.size(); ++index) {
            const ResourceLayer& leftLayer = leftLayers[index];
            const ResourceLayer& rightLayer = rightLayers[index];
            if (leftLayer.sourceName != rightLayer.sourceName
                || leftLayer.source != rightLayer.source
                || leftLayer.entrypoint != rightLayer.entrypoint)
                return false;

            for (const auto& [moduleId, source] : leftLayer.modules) {
                if (!relevantSources.contains(leftLayer.sourceNameFor(moduleId))) continue;
                const auto rightModule = rightLayer.modules.find(moduleId);
                if (rightModule == rightLayer.modules.end() || rightModule->second != source) return false;
            }
            for (const auto& [moduleId, source] : rightLayer.modules) {
                if (!relevantSources.contains(rightLayer.sourceNameFor(moduleId))) continue;
                const auto leftModule = leftLayer.modules.find(moduleId);
                if (leftModule == leftLayer.modules.end() || leftModule->second != source) return false;
            }
        }
    }
    return true;
}
} // namespace

bool SkinReloadCoordinator::sameReloadInputs(const System& system, const ResourceSnapshot& left, const ResourceSnapshot& right) {
    return equalReloadInputs(left, right, system.styleSheet().dependencies());
}

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
        if (std::optional<SkinSnapshotResult> settled = poll(now)) {
            mRequestedSnapshot = std::move(settled);
            mRequested = true;
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
                                         : SkinReloadCoordinator::sameReloadInputs(mSystem, current, *mAcknowledgedSnapshot));
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
    }

    SkinReloadResult reload(SkinSnapshotResult captured, ComponentManager& components) {
        SkinReloadResult result;
        const bool captureOk = captured.ok();
        ResourceSnapshot snapshot = std::move(captured.snapshot);
        result.append(std::move(captured));
        if (!captureOk) {
            result.generationNumber = mSystem.generation();
            return result;
        }

        SkinGenerationPrepareResult prepared = mCompiler.prepare(std::move(snapshot));
        const bool generationOk = prepared.ok();
        std::shared_ptr<const SkinGeneration> generation = std::move(prepared.generation);
        result.append(std::move(prepared));
        if (!generationOk) {
            result.generationNumber = mSystem.generation();
            return result;
        }

        ComponentManager::ReplacementResult replacement = components.prepareReplacement(*generation, mSystem.activeLocale());
        const bool replacementOk = replacement.ok();
        result.append(std::move(replacement));
        if (!replacementOk) {
            result.generationNumber = mSystem.generation();
            return result;
        }

        const bool committed = mSystem.publish(std::move(generation), replacement.replacement);
        if (!committed) {
            result.error("floater.host.replace_failed", "The Floater host rejected the prepared replacements.");
            result.generationNumber = mSystem.generation();
            return result;
        }
        result.committed = true;
        result.generationNumber = mSystem.generation();
        return result;
    }

    System& mSystem;
    const SkinSnapshotSource& mSnapshotSource;
    SkinCompiler mCompiler;
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
