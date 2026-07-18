#ifndef LL_RDUI_FLOATER_PLACEMENT_STORE_H
#define LL_RDUI_FLOATER_PLACEMENT_STORE_H

#include "llsd.h"

#include <optional>
#include <string>
#include <unordered_set>
#include <utility>
#include <variant>

namespace rdui::viewer
{
    class FloaterInstanceId final
    {
        public:
            explicit FloaterInstanceId(std::string value) : mValue(std::move(value)) {}

            const std::string& value() const { return mValue; }
            bool empty() const { return mValue.empty(); }

        private:
            std::string mValue;
    };

    struct FloaterPlacementSize
    {
        float width = 0.f;
        float height = 0.f;
    };

    struct AttachedFloaterPlacement
    {
        float x = 0.f;
        float y = 0.f;
        std::optional<FloaterPlacementSize> size;
    };

    struct DetachedFloaterPlacement
    {
        int x = 0;
        int y = 0;
        int width = 0;
        int height = 0;
        std::string monitor;
        std::optional<FloaterPlacementSize> logicalSize;
    };

    using FloaterPlacement = std::variant<AttachedFloaterPlacement, DetachedFloaterPlacement>;

    class FloaterPlacementStore final
    {
        public:
            class Persistence
            {
                public:
                    virtual ~Persistence() = default;
                    virtual LLSD read() const = 0;
                    virtual void write(const LLSD& placements) = 0;
            };

            explicit FloaterPlacementStore(Persistence& persistence);

            // Restore opens this identity's persistence gate. Saves made before
            // that identity's first restore are deliberately ignored so startup
            // layout cannot overwrite the placement it is about to restore.
            std::optional<FloaterPlacement> restore(const FloaterInstanceId& identity);
            void save(const FloaterInstanceId& identity, FloaterPlacement placement);

        private:
            Persistence& mPersistence;
            std::unordered_set<std::string> mRestoredIdentities;
    };
}

#endif // LL_RDUI_FLOATER_PLACEMENT_STORE_H
