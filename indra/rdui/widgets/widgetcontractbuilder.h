/**
 * @file widgetcontractbuilder.h
 * @brief Template helpers for declaring Widget contracts.
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

#ifndef RD_WIDGETS_WIDGETCONTRACTBUILDER_H
#define RD_WIDGETS_WIDGETCONTRACTBUILDER_H

#include <initializer_list>
#include <type_traits>
#include <utility>
#include <vector>
#include "llerror.h"
#include "widgets/widgetcontract.h"

namespace rdui {
namespace detail {
template<typename PartT, typename OwnerT, typename SlotT> CompositePartContract makeWidgetPart(std::string path, WidgetRef<SlotT> OwnerT::* slot) {
    static_assert(std::is_base_of_v<Widget, OwnerT>);
    static_assert(std::is_base_of_v<Widget, PartT>);
    static_assert(std::is_base_of_v<SlotT, PartT>);

    CompositePartContract contract;
    const std::size_t separator = path.rfind("::");
    if (separator != std::string::npos) contract.parentPath = path.substr(0, separator);
    contract.path = std::move(path);
    contract.create = [] { return std::make_unique<PartT>(); };
    contract.bind = [slot](Widget& owner, Widget& part) {
        OwnerT& typedOwner = static_cast<OwnerT&>(owner);
        PartT& typedPart = static_cast<PartT&>(part);
        (typedOwner.*slot).set(&typedPart);
    };
    return contract;
}
} // namespace detail

inline WidgetAttributeContract allowedAttribute(std::string name) {
    WidgetAttributeContract contract;
    contract.names.push_back(std::move(name));
    return contract;
}

template<typename WidgetT, typename SetterT> WidgetAttributeContract stringAttribute(std::initializer_list<std::string> names, SetterT setter) {
    WidgetAttributeContract contract;
    contract.names.assign(names.begin(), names.end());
    contract.apply = [names = contract.names, setter](const LayoutElement& element, Widget& widget, LayoutBuildResult&, const std::string&,
                                                      const LayoutBuildContext*) {
        std::string value;
        for (const std::string& name : names) {
            if (!readLayoutAttribute(element, name.c_str(), value)) continue;
            (void)std::invoke(setter, static_cast<WidgetT&>(widget), std::move(value));
            return;
        }
    };
    return contract;
}

template<typename WidgetT, typename SetterT> WidgetAttributeContract stringAttribute(std::string name, SetterT setter) {
    return stringAttribute<WidgetT>({std::move(name)}, setter);
}

template<typename WidgetT>
WidgetAttributeContract stringAttribute(std::initializer_list<std::string> names, WidgetT& (WidgetT::*setter)(std::string)) {
    return stringAttribute<WidgetT, decltype(setter)>(names, setter);
}

template<typename WidgetT> WidgetAttributeContract stringAttribute(std::string name, WidgetT& (WidgetT::*setter)(std::string)) {
    return stringAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
}

template<typename WidgetT, typename SetterT> WidgetAttributeContract booleanAttribute(std::string name, SetterT setter) {
    WidgetAttributeContract contract;
    contract.names.push_back(std::move(name));
    contract.apply = [name = contract.names.front(), setter](const LayoutElement& element, Widget& widget, LayoutBuildResult& result,
                                                             const std::string& source, const LayoutBuildContext*) {
        bool value = false;
        if (readLayoutBoolean(element, name.c_str(), value, result, source)) (void)std::invoke(setter, static_cast<WidgetT&>(widget), value);
    };
    return contract;
}

template<typename WidgetT> WidgetAttributeContract booleanAttribute(std::string name, WidgetT& (WidgetT::*setter)(bool)) {
    return booleanAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
}

template<typename WidgetT, typename SetterT> WidgetAttributeContract localizedStringAttribute(std::string name, SetterT setter) {
    WidgetAttributeContract contract;
    contract.names.push_back(std::move(name));
    contract.apply = [name = contract.names.front(), setter](const LayoutElement& element, Widget& widget, LayoutBuildResult& result,
                                                             const std::string& source, const LayoutBuildContext* context) {
        std::string key;
        if (!readLayoutAttribute(element, name.c_str(), key)) return;
        TextSource value = TextSource::text(key);
        if (context) {
            if (!context->hasLocalizationKey(key))
                result.error("layout.localization.missing", "Unknown localization key: " + key + ".", source, element.source().begin.line,
                             element.source().begin.column);
            value = context->localizeContent(key);
        }
        (void)std::invoke(setter, static_cast<WidgetT&>(widget), std::move(value));
    };
    return contract;
}

template<typename WidgetT> WidgetAttributeContract localizedStringAttribute(std::string name, WidgetT& (WidgetT::*setter)(TextSource)) {
    return localizedStringAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
}

template<typename WidgetT> class WidgetContractBuilder final {
public:
    explicit WidgetContractBuilder(std::string elementName) {
        mContract.elementName = std::move(elementName);
        mContract.create = [] { return std::make_unique<WidgetT>(); };
    }
    WidgetContractBuilder(const WidgetContractBuilder&) = delete;
    WidgetContractBuilder& operator=(const WidgetContractBuilder&) = delete;
    WidgetContractBuilder(WidgetContractBuilder&&) noexcept = default;
    WidgetContractBuilder& operator=(WidgetContractBuilder&&) noexcept = default;

    WidgetContractBuilder&& attributes(std::initializer_list<WidgetAttributeContract> attributes) {
        mAttributes.insert(mAttributes.end(), attributes.begin(), attributes.end());
        return std::move(*this);
    }

    template<typename ValidatorT> WidgetContractBuilder&& validate(ValidatorT validator) {
        mValidators.emplace_back(std::move(validator));
        return std::move(*this);
    }

    template<typename ValidatorT> WidgetContractBuilder&& composition(ValidatorT validator) {
        mContract.compositionBehavior.validate = [validator = std::move(validator)](const LayoutElement& element, Widget& widget,
                                                                                    const WidgetScopeContext& scope, LayoutBuildResult& result,
                                                                                    const std::string& source) {
            validator(element, static_cast<WidgetT&>(widget), scope, result, source);
        };
        return std::move(*this);
    }

    WidgetContractBuilder&& events(std::initializer_list<WidgetEventKind> eventKinds) {
        mContract.supportedEvents.assign(eventKinds.begin(), eventKinds.end());
        return std::move(*this);
    }

    WidgetContractBuilder&& state(WidgetState state) {
        mContract.producedStates.push_back(state);
        return std::move(*this);
    }

    WidgetContractBuilder&& labelable() {
        mContract.labelable = true;
        return std::move(*this);
    }

    WidgetContractBuilder&& scopedOnly() {
        mContract.scopedOnly = true;
        return std::move(*this);
    }

    WidgetContractBuilder&& resourceRoot(std::string expectedElementName = {}) {
        if (expectedElementName.empty()) expectedElementName = mContract.elementName;
        mContract.resourceRoot = ResourceRootContract{std::move(expectedElementName)};
        return std::move(*this);
    }

    template<typename PartT, typename OwnerT, typename SlotT>
    WidgetContractBuilder&& part(std::string path, WidgetRef<SlotT> OwnerT::* slot, bool eager = true) {
        static_assert(std::is_same_v<OwnerT, WidgetT>);
        CompositePartContract part = detail::makeWidgetPart<PartT>(std::move(path), slot);
        part.eager = eager;
        mContract.compositeParts.push_back(std::move(part));
        return std::move(*this);
    }

    template<typename OwnerT, typename PartT> WidgetContractBuilder&& part(std::string path, WidgetRef<PartT> OwnerT::* slot, bool eager = true) {
        return part<PartT>(std::move(path), slot, eager);
    }

    WidgetContractBuilder&& textChildren() {
        mContract.contentBehavior.mode = WidgetTextContentMode::TextChildren;
        return std::move(*this);
    }

    template<typename CreateT> WidgetContractBuilder&& textChildren(CreateT create) {
        mContract.contentBehavior.mode = WidgetTextContentMode::TextChildren;
        mContract.contentBehavior.createTextChild = [create = std::move(create)](TextSource content) -> std::unique_ptr<Widget> {
            return create(std::move(content));
        };
        return std::move(*this);
    }

    template<typename ApplyT> WidgetContractBuilder&& widgetText(ApplyT apply) {
        mContract.contentBehavior.mode = WidgetTextContentMode::WidgetText;
        mContract.contentBehavior.applyText = [apply = std::move(apply)](std::string value, Widget& widget, LayoutBuildResult& result,
                                                                         const std::string& source, const LayoutBuildContext* context,
                                                                         std::size_t line) {
            apply(std::move(value), static_cast<WidgetT&>(widget), result, source, context, line);
        };
        return std::move(*this);
    }

    template<typename ApplyT> WidgetContractBuilder&& inlineContent(std::initializer_list<InlineContentKind> accepted, ApplyT apply) {
        mContract.contentBehavior.mode = WidgetTextContentMode::InlineContent;
        mContract.contentBehavior.acceptedInlineContent.assign(accepted.begin(), accepted.end());
        mContract.contentBehavior.applyInlineContent = [apply = std::move(apply)](TextSource content, Widget& widget) {
            apply(std::move(content), static_cast<WidgetT&>(widget));
        };
        return std::move(*this);
    }

    template<typename ApplyT>
    WidgetContractBuilder&& scopedInlineContent(std::string elementName, std::initializer_list<InlineContentKind> accepted, ApplyT apply) {
        ScopedInlineContentContract contract;
        contract.elementName = std::move(elementName);
        contract.accepted.assign(accepted.begin(), accepted.end());
        contract.apply = [apply = std::move(apply)](TextSource content, Widget& widget, LayoutBuildResult& result, const std::string& source,
                                                    std::size_t line, std::size_t column) -> Widget* {
            return apply(std::move(content), static_cast<WidgetT&>(widget), result, source, line, column);
        };
        mContract.contentBehavior.scopedInlineContent.emplace(schemaNameKey(contract.elementName), std::move(contract));
        return std::move(*this);
    }

    template<typename ClaimT> WidgetContractBuilder&& childContainer(std::string name, std::vector<std::string> attributes, ClaimT claim) {
        ChildContainer container;
        container.name = std::move(name);
        container.attributes = std::move(attributes);
        container.claim = std::move(claim);
        mChildContainers.push_back(std::move(container));
        return std::move(*this);
    }

    WidgetContract build() && {
        llassert_always(!mBuilt);
        if (mBuilt) return {};
        mBuilt = true;
        for (const WidgetAttributeContract& attribute : mAttributes)
            mContract.attributes.insert(mContract.attributes.end(), attribute.names.begin(), attribute.names.end());
        if (!mAttributes.empty() || !mValidators.empty()) {
            mContract.attributeBehavior.apply = [attributes = std::move(mAttributes), validators = std::move(mValidators)](
                                                    const LayoutElement& element, Widget& widget, LayoutBuildResult& result,
                                                    const std::string& source, const LayoutBuildContext* context) {
                for (const WidgetAttributeContract& attribute : attributes)
                    if (attribute.apply) attribute.apply(element, widget, result, source, context);
                for (const Validator& validator : validators) validator(element, static_cast<WidgetT&>(widget), result, source, context);
            };
        }
        for (const ChildContainer& container : mChildContainers)
            mContract.childrenBehavior.partAttributes.emplace(schemaNameKey(container.name), container.attributes);
        if (!mChildContainers.empty()) {
            mContract.childrenBehavior.claim = [containers = std::move(mChildContainers)](const LayoutElement& child, Widget& widget,
                                                                                          LayoutBuildResult& result,
                                                                                          const std::string& source) -> ChildClaim {
                for (const ChildContainer& container : containers) {
                    if (schemaNameKey(container.name) != schemaNameKey(child.name())) continue;
                    Widget* target = container.claim(child, static_cast<WidgetT&>(widget), result, source);
                    return target ? ChildClaim::routeTo(*target) : ChildClaim::handled();
                }
                return ChildClaim::notHandled();
            };
        }
        return std::move(mContract);
    }

private:
    using Validator = std::function<void(const LayoutElement&, WidgetT&, LayoutBuildResult&, const std::string&, const LayoutBuildContext*)>;
    struct ChildContainer {
        std::string name;
        std::vector<std::string> attributes;
        std::function<Widget*(const LayoutElement&, WidgetT&, LayoutBuildResult&, const std::string&)> claim;
    };

    WidgetContract mContract;
    std::vector<WidgetAttributeContract> mAttributes;
    std::vector<Validator> mValidators;
    std::vector<ChildContainer> mChildContainers;
    bool mBuilt = false;
};

template<typename WidgetT> WidgetContractBuilder<WidgetT> defineWidget(std::string elementName) {
    return WidgetContractBuilder<WidgetT>(std::move(elementName));
}
} // namespace rdui
#endif // RD_WIDGETS_WIDGETCONTRACTBUILDER_H
