#ifndef LL_RDUI_ICON_H
#define LL_RDUI_ICON_H

#include "rduiwidget.h"

namespace rdui
{

    class Icon : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "icon";

            explicit Icon(std::string name = {});

            Icon& setName(std::string name);
            const std::string& name() const { return mName; }

            void paint(PaintContext& context, const Style& style, float scale) const override;

        private:
            std::string mName;
    };
}

#endif // LL_RDUI_ICON_H
