#ifndef LL_RDUI_FIELD_H
#define LL_RDUI_FIELD_H

#include "rduiwidget.h"

namespace rdui
{
    class Field : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "field";

            Field();

        protected:
            void constrainResolvedStyle(Style& style) const override;
    };

    class Content : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "content";

            Content();

        protected:
            void constrainResolvedStyle(Style& style) const override;
    };

    class Description : public Widget
    {
        friend class detail::WidgetContractRegistry;
        public:
            static constexpr const char* ELEMENT = "description";

            Description();

        protected:
            void constrainResolvedStyle(Style& style) const override;
    };

}

#endif // LL_RDUI_FIELD_H
