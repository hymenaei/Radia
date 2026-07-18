#ifndef LL_RDUI_SKIN_RESOURCES_H
#define LL_RDUI_SKIN_RESOURCES_H

#include "rduiskinresolver.h"

namespace rdui::viewer
{
    class SkinResources final : public SkinSnapshotSource
    {
        public:
            SkinSnapshotResult capture() const override;
            SkinSnapshotResult captureBundledDefault() const;
            bool selectedIsBundledDefault() const;
    };

}

#endif // LL_RDUI_SKIN_RESOURCES_H
