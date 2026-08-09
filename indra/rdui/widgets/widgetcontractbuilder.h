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
    if (separator != std::string::npos) contract.parent_path = path.substr(0, separator);
    contract.path = std::move(path);
    contract.create = [] { return std::make_unique<PartT>(); };
    contract.bind = [slot](Widget& owner, Widget& part) {
        OwnerT& typed_owner = static_cast<OwnerT&>(owner);
        PartT& typed_part = static_cast<PartT&>(part);
        (typed_owner.*slot).set(&typed_part);
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
    contract.apply = [names = contract.names, setter](const LayoutElement& element, Widget& widget, ViewBuildResult&, const std::string&,
                                                      const ViewBuildContext*) {
        std::string value;
        for (const std::string& name : names) {
            if (!readViewAttribute(element, name.c_str(), value)) continue;
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
    contract.apply = [name = contract.names.front(), setter](const LayoutElement& element, Widget& widget, ViewBuildResult& result,
                                                             const std::string& source, const ViewBuildContext*) {
        bool value = false;
        if (readViewBoolean(element, name.c_str(), value, result, source)) (void)std::invoke(setter, static_cast<WidgetT&>(widget), value);
    };
    return contract;
}

template<typename WidgetT> WidgetAttributeContract booleanAttribute(std::string name, WidgetT& (WidgetT::*setter)(bool)) {
    return booleanAttribute<WidgetT, decltype(setter)>(std::move(name), setter);
}

template<typename WidgetT, typename SetterT> WidgetAttributeContract localizedStringAttribute(std::string name, SetterT setter) {
    WidgetAttributeContract contract;
    contract.names.push_back(std::move(name));
    contract.apply = [name = contract.names.front(), setter](const LayoutElement& element, Widget& widget, ViewBuildResult& result,
                                                             const std::string& source, const ViewBuildContext* context) {
        std::string key;
        if (!readViewAttribute(element, name.c_str(), key)) return;
        TextSource value = TextSource::text(key);
        if (context) {
            if (!context->hasLocalizationKey(key))
                result.error("view.localization.missing", "Unknown localization key: " + key + ".", source, element.source().begin.line,
                             element.source().begin.column);
            value = context->localizedContent(key);
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
    explicit WidgetContractBuilder(std::string element) {
        mContract.element = std::move(element);
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
        mContract.composition_behavior.validate = [validator = std::move(validator)](const LayoutElement& element, Widget& widget,
                                                                                     const ViewScopeContext& scope, ViewBuildResult& result,
                                                                                     const std::string& source) {
            validator(element, static_cast<WidgetT&>(widget), scope, result, source);
        };
        return std::move(*this);
    }

    WidgetContractBuilder&& actions(std::initializer_list<ActionEventKind> actions) {
        mContract.supported_actions.assign(actions.begin(), actions.end());
        return std::move(*this);
    }

    WidgetContractBuilder&& state(WidgetState state) {
        mContract.produced_states.push_back(state);
        return std::move(*this);
    }

    WidgetContractBuilder&& labelable() {
        mContract.labelable = true;
        return std::move(*this);
    }

    WidgetContractBuilder&& scopedOnly() {
        mContract.scoped_only = true;
        return std::move(*this);
    }

    WidgetContractBuilder&& resourceRoot(std::string expected_element = {}) {
        if (expected_element.empty()) expected_element = mContract.element;
        mContract.resource_root = ResourceRootContract{std::move(expected_element)};
        return std::move(*this);
    }

    template<typename PartT, typename OwnerT, typename SlotT>
    WidgetContractBuilder&& part(std::string path, WidgetRef<SlotT> OwnerT::* slot, bool eager = true) {
        static_assert(std::is_same_v<OwnerT, WidgetT>);
        CompositePartContract part = detail::makeWidgetPart<PartT>(std::move(path), slot);
        part.eager = eager;
        mContract.composite_parts.push_back(std::move(part));
        return std::move(*this);
    }

    template<typename OwnerT, typename PartT> WidgetContractBuilder&& part(std::string path, WidgetRef<PartT> OwnerT::* slot, bool eager = true) {
        return part<PartT>(std::move(path), slot, eager);
    }

    WidgetContractBuilder&& textChildren() {
        mContract.content_behavior.mode = ViewTextContent::Children;
        return std::move(*this);
    }

    template<typename CreateT> WidgetContractBuilder&& textChildren(CreateT create) {
        mContract.content_behavior.mode = ViewTextContent::Children;
        mContract.content_behavior.create_text_child = [create = std::move(create)](TextSource content) -> std::unique_ptr<Widget> {
            return create(std::move(content));
        };
        return std::move(*this);
    }

    template<typename ApplyT> WidgetContractBuilder&& widgetText(ApplyT apply) {
        mContract.content_behavior.mode = ViewTextContent::Widget;
        mContract.content_behavior.apply_text = [apply = std::move(apply)](std::string value, Widget& widget, ViewBuildResult& result,
                                                                           const std::string& source, const ViewBuildContext* context,
                                                                           std::size_t line) {
            apply(std::move(value), static_cast<WidgetT&>(widget), result, source, context, line);
        };
        return std::move(*this);
    }

    template<typename ApplyT> WidgetContractBuilder&& inlineContent(std::initializer_list<InlineContentKind> accepted, ApplyT apply) {
        mContract.content_behavior.mode = ViewTextContent::Inline;
        mContract.content_behavior.accepted_inline_content.assign(accepted.begin(), accepted.end());
        mContract.content_behavior.apply_inline_content = [apply = std::move(apply)](TextSource content, Widget& widget) {
            apply(std::move(content), static_cast<WidgetT&>(widget));
        };
        return std::move(*this);
    }

    template<typename ApplyT>
    WidgetContractBuilder&& scopedInlineContent(std::string element, std::initializer_list<InlineContentKind> accepted, ApplyT apply) {
        ScopedInlineContentContract contract;
        contract.element = std::move(element);
        contract.accepted.assign(accepted.begin(), accepted.end());
        contract.apply = [apply = std::move(apply)](TextSource content, Widget& widget, ViewBuildResult& result, const std::string& source,
                                                    std::size_t line, std::size_t column) -> Widget* {
            return apply(std::move(content), static_cast<WidgetT&>(widget), result, source, line, column);
        };
        mContract.content_behavior.scoped_inline_content.emplace(schemaNameKey(contract.element), std::move(contract));
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
            mContract.attribute_behavior.apply = [attributes = std::move(mAttributes), validators = std::move(mValidators)](
                                                     const LayoutElement& element, Widget& widget, ViewBuildResult& result, const std::string& source,
                                                     const ViewBuildContext* context) {
                for (const WidgetAttributeContract& attribute : attributes)
                    if (attribute.apply) attribute.apply(element, widget, result, source, context);
                for (const Validator& validator : validators) validator(element, static_cast<WidgetT&>(widget), result, source, context);
            };
        }
        for (const ChildContainer& container : mChildContainers)
            mContract.children_behavior.part_attributes.emplace(schemaNameKey(container.name), container.attributes);
        if (!mChildContainers.empty()) {
            mContract.children_behavior.claim = [containers = std::move(mChildContainers)](const LayoutElement& child, Widget& widget,
                                                                                           ViewBuildResult& result,
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
    using Validator = std::function<void(const LayoutElement&, WidgetT&, ViewBuildResult&, const std::string&, const ViewBuildContext*)>;
    struct ChildContainer {
        std::string name;
        std::vector<std::string> attributes;
        std::function<Widget*(const LayoutElement&, WidgetT&, ViewBuildResult&, const std::string&)> claim;
    };

    WidgetContract mContract;
    std::vector<WidgetAttributeContract> mAttributes;
    std::vector<Validator> mValidators;
    std::vector<ChildContainer> mChildContainers;
    bool mBuilt = false;
};

template<typename WidgetT> WidgetContractBuilder<WidgetT> defineWidget(std::string element) {
    return WidgetContractBuilder<WidgetT>(std::move(element));
}
} // namespace rdui
#endif // RD_WIDGETS_WIDGETCONTRACTBUILDER_H
