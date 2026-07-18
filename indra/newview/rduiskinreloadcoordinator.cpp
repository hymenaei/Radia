#include "llviewerprecompiledheaders.h"
#include "rduiskinreloadcoordinator.h"

#include "rdfloater.h"
#include "rduiskincompiler.h"
#include "rduiskinresolver.h"
#include "rduiskingeneration.h"
#include "rduisystem.h"

#include <algorithm>
#include <utility>

namespace rdui::viewer
{
    namespace
    {
        constexpr std::chrono::milliseconds SCAN_INTERVAL{250};
        constexpr std::chrono::milliseconds SETTLE_INTERVAL{150};

        bool equalSnapshots(const ResourceSnapshot& left, const ResourceSnapshot& right)
        {
            return left.resources() == right.resources()
                && left.layeredResources() == right.layeredResources();
        }

        struct PendingDocument
        {
            std::unique_ptr<Floater> floater;
            PreparedBinding binding;
            ReloadableFloater* document = nullptr;
            std::function<void(std::unique_ptr<Floater>)> install;
        };

        struct PendingCommit
        {
            std::vector<PendingDocument> documents;
            bool committed = false;
        };
    }

    class SkinReloadCoordinator::Impl
    {
        public:
            Impl(System& system, const SkinSnapshotSource& snapshots)
                : mSystem(system), mSnapshots(snapshots) {}

            void setAuthoringEnabled(bool enabled)
            {
                if (mAuthoringEnabled == enabled) return;
                mAuthoringEnabled = enabled;
                mObserved.reset();
                mAcknowledged.reset();
                mNextScan = {};
                mLastChange = {};
                mSettling = false;
                mRetryAfterAnyChange = false;
            }

            void request()
            {
                mRequested = true;
                mRequestedSnapshot.reset();
            }

            std::optional<SkinReloadResult> update(
                TimePoint now, const std::vector<FloaterReloadTarget>& targets)
            {
                if (std::optional<SkinSnapshotResult> settled = poll(now))
                {
                    mRequestedSnapshot = std::move(settled);
                    mRequested = true;
                }
                if (!mRequested) return std::nullopt;

                mRequested = false;
                SkinSnapshotResult captured = mRequestedSnapshot
                    ? std::move(*mRequestedSnapshot) : mSnapshots.capture();
                mRequestedSnapshot.reset();
                acknowledge(captured.snapshot, now);
                SkinReloadResult result = reload(std::move(captured), targets);
                mRetryAfterAnyChange = !result.ok();
                return result;
            }

        private:
            std::optional<SkinSnapshotResult> poll(TimePoint now)
            {
                if (!mAuthoringEnabled) return std::nullopt;
                if (mNextScan != TimePoint() && now < mNextScan) return std::nullopt;
                mNextScan = now + SCAN_INTERVAL;

                SkinSnapshotResult captured = mSnapshots.capture();
                ResourceSnapshot& current = captured.snapshot;
                if (!mObserved)
                {
                    mObserved = current;
                    mAcknowledged = current;
                    return std::nullopt;
                }
                if (!mSettling)
                {
                    const bool unchanged = mAcknowledged
                        && (mRetryAfterAnyChange
                            ? equalSnapshots(current, *mAcknowledged)
                            : mSystem.sameReloadInputs(current, *mAcknowledged));
                    if (unchanged)
                    {
                        // Unreferenced modules are allowed to drift without
                        // becoming latent changes in the next candidate.
                        mObserved = current;
                        mAcknowledged = current;
                        return std::nullopt;
                    }
                    mObserved = std::move(current);
                    mLastChange = now;
                    mSettling = true;
                    return std::nullopt;
                }
                if (!equalSnapshots(current, *mObserved))
                {
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

            void acknowledge(const ResourceSnapshot& snapshot, TimePoint now)
            {
                if (!mAuthoringEnabled) return;
                mObserved = snapshot;
                mAcknowledged = snapshot;
                mLastChange = now;
                mSettling = false;
            }

            SkinReloadResult reload(SkinSnapshotResult captured,
                                    const std::vector<FloaterReloadTarget>& targets)
            {
                SkinReloadResult result;
                if (std::any_of(targets.begin(), targets.end(), [](const FloaterReloadTarget& target)
                    { return !target.document || !target.install; }))
                {
                    result.error("reload.target.invalid", "Reload targets require a document and installation callback.");
                    result.generation = mSystem.generation();
                    return result;
                }

                const bool capture_ok = captured.ok();
                result.append(std::move(captured));
                if (!capture_ok)
                {
                    result.generation = mSystem.generation();
                    return result;
                }

                SkinGenerationPrepareResult prepared = mCompiler.prepare(std::move(captured.snapshot));
                const bool generation_ok = prepared.ok();
                result.append(std::move(prepared));
                if (!generation_ok)
                {
                    result.generation = mSystem.generation();
                    return result;
                }

                auto pending = std::make_shared<PendingCommit>();
                pending->documents.reserve(targets.size());
                for (const FloaterReloadTarget& target : targets)
                {
                    ReloadableFloater& document = *target.document;
                    const std::string resource_id = document.reloadResourceId();
                    ViewBuildResult view = prepared.generation->createView(resource_id, mSystem.activeLocale());
                    Floater* candidate = view.rootAs<Floater>();
                    if (view.ok() && !candidate)
                        view.error("view.root.type_mismatch", "Reloaded View must have a <floater> root.", resource_id);
                    const bool view_ok = view.ok() && candidate;
                    result.append(std::move(view));
                    if (!view_ok)
                    {
                        result.generation = mSystem.generation();
                        return result;
                    }

                    PreparedBindingResult binding = document.prepareBindings(*candidate);
                    const bool binding_ok = binding.ok();
                    result.append(std::move(binding));
                    if (!binding_ok)
                    {
                        result.generation = mSystem.generation();
                        return result;
                    }

                    pending->documents.push_back({
                        std::unique_ptr<Floater>(static_cast<Floater*>(view.root.release())),
                        std::move(binding.binding), &document, target.install});
                }

                mSystem.publish(std::move(prepared.generation), [pending]
                {
                    for (PendingDocument& document : pending->documents)
                        document.install(std::move(document.floater));
                    for (PendingDocument& document : pending->documents)
                        document.document->commitBindings(std::move(document.binding));
                    pending->committed = true;
                });
                result.committed = pending->committed;
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

    void SkinReloadCoordinator::setAuthoringEnabled(bool enabled)
    {
        mImpl->setAuthoringEnabled(enabled);
    }

    void SkinReloadCoordinator::request()
    {
        mImpl->request();
    }

    std::optional<SkinReloadResult> SkinReloadCoordinator::update(
        TimePoint now, const std::vector<FloaterReloadTarget>& targets)
    {
        return mImpl->update(now, targets);
    }
}
