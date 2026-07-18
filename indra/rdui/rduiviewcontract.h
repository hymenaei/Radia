#ifndef LL_RDUI_VIEW_CONTRACT_H
#define LL_RDUI_VIEW_CONTRACT_H

#include "rduidiagnostic.h"
#include "rduilayoutdocument.h"
#include "rduilocalization.h"
#include "rduiviewresult.h"
#include <cstdint>
#include <functional>
#include <initializer_list>
#include <optional>
#include <string>
#include <type_traits>
#include <unordered_map>
#include <utility>
#include <vector>

namespace rdui
{
    class ViewBuildContext
    {
        public:
            ViewBuildContext(const LocalizationCatalog& localization, std::string locale)
                : mLocalization(localization), mLocale(std::move(locale)) {}

            bool hasLocalizationKey(const std::string& id) const
            {
                return mLocalization.containsDefaultString(id);
            }

            std::string resolveText(const std::string& id) const
            {
                return mLocalization.get(mLocale, id);
            }

            TextValue localized(std::string id) const
            {
                std::string value = resolveText(id);
                return TextValue::fromLocalization(std::move(id), std::move(value));
            }

        private:
            const LocalizationCatalog& mLocalization;
            std::string mLocale;
    };

    enum class ViewTextContent : uint8_t
    {
        Unsupported,
        Widget,
        Children,
    };

    struct CompositePartContract
    {
        CompositePartContract() = default;

        std::string path;
        std::string parent_path;
        std::function<std::unique_ptr<Widget>()> create;
        std::function<void(Widget&, Widget&)> bind;
        std::vector<WidgetState> produced_states;
        bool eager = true;
    };

    namespace detail
    {
        template<typename PartT, typename OwnerT, typename SlotT>
        CompositePartContract makeWidgetPart(std::string path, WidgetRef<SlotT> OwnerT::* slot)
        {
            static_assert(std::is_base_of_v<Widget, OwnerT>);
            static_assert(std::is_base_of_v<Widget, PartT>);
            static_assert(std::is_base_of_v<SlotT, PartT>);

            CompositePartContract contract;
            const std::size_t separator = path.rfind("::");
            if (separator != std::string::npos) contract.parent_path = path.substr(0, separator);
            contract.path = std::move(path);
            contract.create = [] { return std::make_unique<PartT>(); };
            contract.bind = [slot](Widget& owner, Widget& part)
            {
                OwnerT& typed_owner = static_cast<OwnerT&>(owner);
                PartT& typed_part = static_cast<PartT&>(part);
                (typed_owner.*slot).set(&typed_part);
            };
            return contract;
        }
    }

    bool readViewAttribute(const LayoutElement& element, const char* name, std::string& value);
    bool readViewBoolean(const LayoutElement& element, const char* name, bool& value, ViewBuildResult& result, const std::string& source);

    struct WidgetAttributeContract
    {
        std::vector<std::string> names;
        std::function<void(const LayoutElement&, Widget&, ViewBuildResult&, const std::string&, const ViewBuildContext*)> apply;
    };

    inline WidgetAttributeContract allowedAttribute(std::string name)
    {
        WidgetAttributeContract contract;
        contract.names.push_back(std::move(name));
        return contract;
    }

    template<typename WidgetT, typename SetterT>
    WidgetAttributeContract stringAttribute(std::initializer_list<std::string> names, SetterT setter)
    {
        WidgetAttributeContract contract;
        contract.names.assign(names.begin(), names.end());
        contract.apply = [names = contract.names, setter](const LayoutElement& element, Widget& widget,
                                                          ViewBuildResult&, const std::string&, const ViewBuildContext*)
        {
            std::string value;
            for (const std::string& name : names)
            {
                if (!readViewAttribute(element, name.c_str(), value)) continue;
                (void)std::invoke(setter, static_cast<WidgetT&>(widget), std::move(value));
                return;
            }
        };
        return contract;
    }

    template<typename WidgetT, typename SetterT>
    WidgetAttributeContract stringAttribute(std::string name, SetterT setter)
    {
        return stringAttribute<WidgetT>({std::move(name)}, setter);
    }

    template<typename WidgetT>
    WidgetAttributeContract stringAttribute(std::initializer_list<std::string> names,
                                            WidgetT& (WidgetT::*setter)(std::string))
    {
        return stringAttribute<WidgetT, decltype(setter)>(names, setter);
    }

    template<typename WidgetT>
    WidgetAttributeContract stringAttribute(std::string name, WidgetT& (WidgetT::*setter)(std::string))
    {
        return stringAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
    }

    template<typename WidgetT, typename SetterT>
    WidgetAttributeContract booleanAttribute(std::string name, SetterT setter)
    {
        WidgetAttributeContract contract;
        contract.names.push_back(std::move(name));
        contract.apply = [name = contract.names.front(), setter](const LayoutElement& element, Widget& widget,
                                                                 ViewBuildResult& result, const std::string& source,
                                                                 const ViewBuildContext*)
        {
            bool value = false;
            if (readViewBoolean(element, name.c_str(), value, result, source))
                (void)std::invoke(setter, static_cast<WidgetT&>(widget), value);
        };
        return contract;
    }

    template<typename WidgetT>
    WidgetAttributeContract booleanAttribute(std::string name, WidgetT& (WidgetT::*setter)(bool))
    {
        return booleanAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
    }

    template<typename WidgetT, typename SetterT>
    WidgetAttributeContract localizedStringAttribute(std::string name, SetterT setter)
    {
        WidgetAttributeContract contract;
        contract.names.push_back(std::move(name));
        contract.apply = [name = contract.names.front(), setter](const LayoutElement& element, Widget& widget,
                                                                 ViewBuildResult& result, const std::string& source,
                                                                 const ViewBuildContext* context)
        {
            std::string key;
            if (!readViewAttribute(element, name.c_str(), key)) return;
            std::string value = key;
            if (context)
            {
                if (!context->hasLocalizationKey(key))
                    result.error("view.localization.missing", "Unknown localization key: " + key + ".",
                                 source, element.source().begin.line, element.source().begin.column);
                value = context->resolveText(key);
            }
            (void)std::invoke(setter, static_cast<WidgetT&>(widget), std::move(key), std::move(value));
        };
        return contract;
    }

    template<typename WidgetT>
    WidgetAttributeContract localizedStringAttribute(
        std::string name, WidgetT& (WidgetT::*setter)(std::string, std::string))
    {
        return localizedStringAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
    }

    struct WidgetContract
    {
        WidgetContract() = default;

        std::string element;
        std::function<std::unique_ptr<Widget>()> create;
        std::vector<std::string> attributes;
        std::unordered_map<std::string, std::vector<std::string>> part_attributes;
        std::function<void(const LayoutElement&, Widget&, ViewBuildResult&, const std::string&, const ViewBuildContext*)> apply_attributes;
        std::vector<ActionEventKind> supported_actions;
        std::function<std::optional<Widget*>(const LayoutElement&, Widget&, ViewBuildResult&, const std::string&)> child_container;
        std::vector<WidgetState> produced_states;
        std::vector<CompositePartContract> composite_parts;
        ViewTextContent text_content = ViewTextContent::Unsupported;
        std::function<void(std::string, Widget&, ViewBuildResult&, const std::string&,
                           const ViewBuildContext*, std::size_t)> apply_text;
    };

    template<typename WidgetT>
    class WidgetContractBuilder final
    {
        public:
            explicit WidgetContractBuilder(std::string element)
            {
                mContract.element = std::move(element);
                mContract.create = [] { return std::make_unique<WidgetT>(); };
            }

            WidgetContractBuilder& attributes(std::initializer_list<WidgetAttributeContract> attributes)
            {
                mAttributes.insert(mAttributes.end(), attributes.begin(), attributes.end());
                return *this;
            }

            template<typename ValidatorT>
            WidgetContractBuilder& validate(ValidatorT validator)
            {
                mValidators.emplace_back(std::move(validator));
                return *this;
            }

            WidgetContractBuilder& actions(std::initializer_list<ActionEventKind> actions)
            {
                mContract.supported_actions.assign(actions.begin(), actions.end());
                return *this;
            }

            WidgetContractBuilder& state(WidgetState state)
            {
                mContract.produced_states.push_back(state);
                return *this;
            }

            template<typename PartT, typename OwnerT, typename SlotT>
            WidgetContractBuilder& part(std::string path, WidgetRef<SlotT> OwnerT::* slot)
            {
                static_assert(std::is_same_v<OwnerT, WidgetT>);
                mContract.composite_parts.push_back(detail::makeWidgetPart<PartT>(std::move(path), slot));
                return *this;
            }

            template<typename OwnerT, typename PartT>
            WidgetContractBuilder& part(std::string path, WidgetRef<PartT> OwnerT::* slot)
            {
                return part<PartT>(std::move(path), slot);
            }

            WidgetContractBuilder& textChildren()
            {
                mContract.text_content = ViewTextContent::Children;
                return *this;
            }

            template<typename ApplyT>
            WidgetContractBuilder& widgetText(ApplyT apply)
            {
                mContract.text_content = ViewTextContent::Widget;
                mContract.apply_text = [apply = std::move(apply)](std::string value, Widget& widget,
                                                                  ViewBuildResult& result, const std::string& source,
                                                                  const ViewBuildContext* context, std::size_t line)
                {
                    apply(std::move(value), static_cast<WidgetT&>(widget), result, source, context, line);
                };
                return *this;
            }

            template<typename ClaimT>
            WidgetContractBuilder& childContainer(std::string name, std::vector<std::string> attributes, ClaimT claim)
            {
                ChildContainer container;
                container.name = std::move(name);
                container.attributes = std::move(attributes);
                container.claim = std::move(claim);
                mChildContainers.push_back(std::move(container));
                return *this;
            }

            WidgetContract build()
            {
                for (const WidgetAttributeContract& attribute : mAttributes)
                    mContract.attributes.insert(mContract.attributes.end(), attribute.names.begin(), attribute.names.end());
                if (!mAttributes.empty() || !mValidators.empty())
                {
                    mContract.apply_attributes = [attributes = std::move(mAttributes), validators = std::move(mValidators)](
                        const LayoutElement& element, Widget& widget, ViewBuildResult& result,
                        const std::string& source, const ViewBuildContext* context)
                    {
                        for (const WidgetAttributeContract& attribute : attributes)
                            if (attribute.apply) attribute.apply(element, widget, result, source, context);
                        for (const Validator& validator : validators)
                            validator(element, static_cast<WidgetT&>(widget), result, source, context);
                    };
                }
                for (const ChildContainer& container : mChildContainers)
                    mContract.part_attributes.emplace(container.name, container.attributes);
                if (!mChildContainers.empty())
                {
                    mContract.child_container = [containers = std::move(mChildContainers)](
                        const LayoutElement& child, Widget& widget, ViewBuildResult& result,
                        const std::string& source) -> std::optional<Widget*>
                    {
                        for (const ChildContainer& container : containers)
                        {
                            if (container.name != child.name()) continue;
                            return container.claim(child, static_cast<WidgetT&>(widget), result, source);
                        }
                        return std::nullopt;
                    };
                }
                return std::move(mContract);
            }

        private:
            using Validator = std::function<void(const LayoutElement&, WidgetT&, ViewBuildResult&,
                                                 const std::string&, const ViewBuildContext*)>;
            struct ChildContainer
            {
                std::string name;
                std::vector<std::string> attributes;
                std::function<Widget*(const LayoutElement&, WidgetT&, ViewBuildResult&, const std::string&)> claim;
            };

            WidgetContract mContract;
            std::vector<WidgetAttributeContract> mAttributes;
            std::vector<Validator> mValidators;
            std::vector<ChildContainer> mChildContainers;
    };

    template<typename WidgetT>
    WidgetContractBuilder<WidgetT> defineWidget(std::string element)
    {
        return WidgetContractBuilder<WidgetT>(std::move(element));
    }

    const std::unordered_map<std::string, WidgetContract>& builtInWidgetContracts();
    const WidgetContract* findWidgetContract(const std::string& element);
    const CompositePartContract* findCompositePartContract(const WidgetContract& widget,
                                                           const std::vector<std::string>& parts);
    bool producesState(const WidgetContract& widget, WidgetState state);
    bool producesState(const CompositePartContract& part, WidgetState state);
    const char* actionAttribute(ActionEventKind kind);
    namespace detail
    {
        class WidgetContractRegistry
        {
            public:
                static WidgetContract button();
                static WidgetContract content();
                static WidgetContract description();
                static WidgetContract field();
                static WidgetContract floater();
                static WidgetContract icon();
                static WidgetContract label();
                static WidgetContract panel();
                static WidgetContract toggleSwitch();
        };

        class WidgetCompilerAccess
        {
            public:
                static void setStyleIdentity(Widget& widget, std::string element, std::string part);
                static void setIdScopeRoot(Widget& widget);
                static void setState(Widget& widget, WidgetState state, bool enabled);
        };

        void instantiateCompositeParts(Widget& owner, const WidgetContract& contract);
        Widget* instantiateCompositePart(Widget& owner, const WidgetContract& contract, const std::string& path);
    }

    TextValue localizedViewText(std::string value, ViewBuildResult& result, const std::string& source,
                                const ViewBuildContext* context, std::size_t line = 0);
    void validateViewAttributes(const LayoutElement& element, const std::vector<std::string>& widget_attributes, ViewBuildResult& result, const std::string& source);
    void applyCommonViewAttributes(const LayoutElement& element, Widget& widget, ViewBuildResult& result,
                                   const std::string& source,
                                   const std::vector<ActionEventKind>& supported_actions = {});
}

#endif // LL_RDUI_VIEW_CONTRACT_H
