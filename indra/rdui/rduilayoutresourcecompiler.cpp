#include "linden_common.h"
#include "rduilayoutresourcecompiler.h"
#include "rduiinlinecontentcompiler.h"
#include "rduiwidgetcatalog.h"
#include "rdbutton.h"
#include "rdfloater.h"
#include "rdicon.h"
#include "rdlabel.h"
#include "rdpanel.h"
#include "rdswitch.h"
#include <algorithm>
#include <cctype>
#include <functional>
#include <unordered_set>

namespace rdui
{
    struct LayoutResourceCompiler::BuildState
    {
        struct AuthoredWidget
        {
            Widget* widget = nullptr;
            const LayoutNode* node = nullptr;
            const LayoutNode* defaults = nullptr;
            const WidgetContract* contract = nullptr;
            std::string source;
        };

        ViewBuildResult result;
        std::vector<std::string> resources;
        std::unordered_map<std::string, const LayoutNode*> widget_defaults;
        std::unordered_map<const Widget*, AuthoredWidget> authored_widgets;
        const ViewBuildContext* context = nullptr;
    };

    namespace
    {
        std::string textKey(const std::string& text)
        {
            const auto first = std::find_if_not(text.begin(), text.end(), [](unsigned char character) { return std::isspace(character); });
            if (first == text.end()) return {};
            const auto last = std::find_if_not(text.rbegin(), text.rend(), [](unsigned char character) { return std::isspace(character); }).base();
            return std::string(first, last);
        }

        std::string directoryOf(const std::string& filename)
        {
            const std::size_t slash = filename.find_last_of('/');
            return slash == std::string::npos ? std::string() : filename.substr(0, slash + 1);
        }

        std::string resourceChain(const std::vector<std::string>& resources, const std::string& filename)
        {
            std::string result;
            for (const std::string& resource : resources)
            {
                if (!result.empty()) result += " -> ";
                result += resource;
            }
            if (!result.empty()) result += " -> ";
            return result + filename;
        }

        bool hasAuthoredContent(const LayoutNode& node)
        {
            for (const LayoutContent& content : node.content)
            {
                if (content.element || !textKey(content.text).empty()) return true;
            }
            return false;
        }

        void rejectLogicalAttributes(const LayoutElement& element, ViewBuildResult& result,
                                     const std::string& source)
        {
            for (const auto& [name, attribute] : element.attributes())
                result.error("view.attribute.unknown",
                             "Unknown attribute on <" + element.name() + ">: "
                             + attribute.authored_name + ".",
                             source, attribute.source.begin.line, attribute.source.begin.column);
        }


    }

    LayoutResourceCompiler::LayoutResourceCompiler(const LayoutDocumentMap* documents)
        : mDocuments(documents), mWidgetContracts(builtInWidgetContracts())
    {
    }

    std::string LayoutResourceCompiler::normalizeResource(std::string filename)
    {
        std::replace(filename.begin(), filename.end(), '\\', '/');
        while (filename.rfind("./", 0) == 0) filename.erase(0, 2);
        if (filename.rfind("xui/", 0) == 0) filename.erase(0, 4);
        if (filename.empty()) return {};

        std::vector<std::string> segments;
        std::size_t start = 0;
        while (start <= filename.size())
        {
            const std::size_t slash = filename.find('/', start);
            const std::string segment = filename.substr(start, slash == std::string::npos ? std::string::npos : slash - start);
            if (segment == "..")
            {
                if (segments.empty()) return {};
                segments.pop_back();
            }
            else if (!segment.empty() && segment != ".") segments.push_back(segment);
            if (slash == std::string::npos) break;
            start = slash + 1;
        }

        std::string result;
        for (const std::string& segment : segments)
        {
            if (!result.empty()) result += '/';
            result += segment;
        }
        return result;
    }

    ViewBuildResult LayoutResourceCompiler::createFromResource(const std::string& filename,
                                                               const ViewBuildContext* context) const
    {
        BuildState state;
        state.context = context;
        const std::string resource = normalizeResource(filename);
        std::unique_ptr<Widget> root = createResourceWidget(resource, state);
        if (root && !state.result.hasErrors())
        {
            detail::WidgetCompilerAccess::setIdScopeRoot(*root);
            validateViewScope(*root, state, resource);
        }
        if (!state.result.hasErrors()) state.result.root = std::move(root);
        return std::move(state.result);
    }

    ViewBuildResult LayoutResourceCompiler::createFromString(const std::string& xml,
                                                             const std::string& source_name,
                                                             const ViewBuildContext* context) const
    {
        ViewBuildResult result;
        const std::string source = normalizeResource(source_name);
        LayoutDocumentParseResult parsed = LayoutDocumentParser().parse(xml, source);
        result.append(std::move(parsed));
        if (!parsed.document) return result;

        BuildState state;
        state.context = context;
        state.result = std::move(result);
        std::unique_ptr<Widget> root = buildDocument(*parsed.document, nullptr, state);
        if (root && !state.result.hasErrors())
        {
            detail::WidgetCompilerAccess::setIdScopeRoot(*root);
            validateViewScope(*root, state, source);
        }
        if (!state.result.hasErrors()) state.result.root = std::move(root);
        return std::move(state.result);
    }

    void LayoutResourceCompiler::validateViewScope(Widget& scope, BuildState& state,
                                                   const std::string& source, bool count_root) const
    {
        std::unordered_map<std::string, Widget*> ids;
        std::unordered_set<std::string> duplicates;
        std::vector<const BuildState::AuthoredWidget*> authored_widgets;
        std::function<void(Widget&, bool)> visit = [&](Widget& widget, bool root)
        {
            if (!root && widget.idScopeRoot())
            {
                if (!widget.id().empty() && !ids.emplace(widget.id(), &widget).second)
                {
                    duplicates.insert(widget.id());
                    state.result.error("view.id.duplicate", "Duplicate widget id: " + widget.id() + ".", source);
                }
                validateViewScope(widget, state, source, false);
                return;
            }
            if ((!root || count_root) && !widget.id().empty() && !ids.emplace(widget.id(), &widget).second)
            {
                duplicates.insert(widget.id());
                state.result.error("view.id.duplicate", "Duplicate widget id: " + widget.id() + ".", source);
            }
            const auto authored = state.authored_widgets.find(&widget);
            if (authored != state.authored_widgets.end()) authored_widgets.push_back(&authored->second);
            for (const auto& child : widget.children()) visit(*child, false);
        };
        visit(scope, true);

        const ViewScopeContext context(ids, duplicates, [this](const Widget& widget)
        {
            const auto contract = mWidgetContracts.find(schemaNameKey(widget.element()));
            return contract != mWidgetContracts.end() && contract->second.labelable;
        });
        for (const BuildState::AuthoredWidget* authored : authored_widgets)
        {
            if (!authored->widget || !authored->node || !authored->contract
                || !authored->contract->validate_composition) continue;
            const LayoutElement element(*authored->node, authored->defaults);
            authored->contract->validate_composition(
                element, *authored->widget, context, state.result, authored->source);
        }
    }

    DiagnosticResult LayoutResourceCompiler::validateWidgetDefaults(const std::string& element,
                                                                     const ViewBuildContext* context) const
    {
        BuildState state;
        state.context = context;
        loadWidgetDefaults(element, state);
        DiagnosticResult result;
        result.warnings = std::move(state.result.warnings);
        result.errors = std::move(state.result.errors);
        return result;
    }

    void LayoutResourceCompiler::loadWidgetDefaults(const std::string& element, BuildState& state) const
    {
        const std::string lookup = schemaNameKey(element);
        if (state.widget_defaults.find(lookup) != state.widget_defaults.end()) return;
        const auto contract = mWidgetContracts.find(lookup);
        const std::string canonical = contract == mWidgetContracts.end() ? element : contract->second.element;
        const std::string default_resource = "widgets/" + canonical + ".xml";
        const LayoutNode* default_root = nullptr;
        if (contract == mWidgetContracts.end())
        {
            state.result.error("view.defaults.element_unknown",
                               "Widget Defaults target an unsupported element: " + canonical + ".",
                               default_resource);
            state.widget_defaults.emplace(lookup, nullptr);
            return;
        }

        if (mDocuments)
        {
            const auto document = mDocuments->find(default_resource);
            if (document == mDocuments->end() || !document->second || !document->second->root)
            {
                state.widget_defaults.emplace(lookup, nullptr);
                return;
            }
            default_root = document->second->root.get();
            const std::size_t errors_before = state.result.errors.size();
            const LayoutElement defaults(*default_root);
            if (schemaNameKey(default_root->name) != lookup)
            {
                state.result.error("view.defaults.root_invalid", "Widget Defaults root must be <" + canonical + ">.", default_resource,
                                   default_root->source.begin.line, default_root->source.begin.column);
                default_root = nullptr;
            }
            else if (hasAuthoredContent(*default_root))
            {
                state.result.error("view.defaults.children_unsupported", "Widget Defaults may contain attributes only.", default_resource,
                                   default_root->source.begin.line, default_root->source.begin.column);
                default_root = nullptr;
            }
            else
            {
                validateViewAttributes(defaults, contract->second.attributes, state.result, default_resource);
                std::string ignored;
                if (readViewAttribute(defaults, "id", ignored) || readViewAttribute(defaults, "filename", ignored)
                    || readViewAttribute(defaults, "for", ignored)
                    || readViewAttribute(defaults, "longClickDelay", ignored))
                {
                    state.result.error("view.defaults.controller_attribute",
                                       "Widget Defaults cannot declare IDs, relationships, includes, or controller behavior.",
                                       default_resource);
                    default_root = nullptr;
                }
                for (ActionEventKind kind : {ActionEventKind::Click, ActionEventKind::DoubleClick, ActionEventKind::Change,
                                             ActionEventKind::MouseDown, ActionEventKind::MouseUp, ActionEventKind::MouseMove,
                                             ActionEventKind::LongClick, ActionEventKind::ContextMenu})
                {
                    if (default_root && readViewAttribute(defaults, actionAttribute(kind), ignored))
                    {
                        state.result.error("view.defaults.controller_attribute", "Widget Defaults cannot declare action handlers.", default_resource);
                        default_root = nullptr;
                        break;
                    }
                }

                if (default_root)
                {
                    std::unique_ptr<Widget> probe = contract->second.create();
                    applyCommonViewAttributes(defaults, *probe, state.result, default_resource,
                                              contract->second.supported_actions);
                    if (contract->second.apply_attributes)
                        contract->second.apply_attributes(defaults, *probe, state.result, default_resource, state.context);
                }
            }
            if (state.result.errors.size() != errors_before) default_root = nullptr;
        }
        state.widget_defaults.emplace(lookup, default_root);
    }

    std::unique_ptr<Widget> LayoutResourceCompiler::createResourceWidget(const std::string& filename, BuildState& state) const
    {
        const std::string resource = normalizeResource(filename);
        if (resource.empty())
        {
            state.result.error("view.resource.path_invalid", "Invalid or empty resource path: " + filename + ".", filename);
            return nullptr;
        }
        if (resource.rfind("widgets/", 0) == 0)
        {
            state.result.error("view.resource.reserved", "Widget default resources cannot be instantiated as Views: " + resource + ".", resource);
            return nullptr;
        }
        if (std::find(state.resources.begin(), state.resources.end(), resource) != state.resources.end())
        {
            state.result.error("view.resource.cycle", "Recursive panel resource reference: " + resourceChain(state.resources, resource) + ".", resource);
            return nullptr;
        }
        if (!mDocuments)
        {
            state.result.error("view.resource.provider_missing", "No Layout Resource collection is configured.", resource);
            return nullptr;
        }
        const auto document = mDocuments->find(resource);
        if (document == mDocuments->end() || !document->second)
        {
            state.result.error("view.resource.missing", "Could not load resource chain: " + resourceChain(state.resources, resource) + ".", resource);
            return nullptr;
        }

        state.resources.push_back(resource);
        std::unique_ptr<Widget> root = buildDocument(*document->second, nullptr, state);
        state.resources.pop_back();
        if (root) detail::WidgetCompilerAccess::setIdScopeRoot(*root);
        return root;
    }

    std::unique_ptr<Widget> LayoutResourceCompiler::buildDocument(const LayoutDocument& document,
                                                                 std::unique_ptr<Widget> root,
                                                                 BuildState& state) const
    {
        if (!document.root)
        {
            state.result.error("view.xml.invalid", "Radia UI XML must contain one root element.", document.source);
            return nullptr;
        }

        std::function<std::unique_ptr<Widget>(const LayoutNode&, const std::string&, std::unique_ptr<Widget>)> buildNode;
        buildNode = [&](const LayoutNode& layout_node, const std::string& current_source,
                        std::unique_ptr<Widget> node) -> std::unique_ptr<Widget>
        {
            const std::string lookup = schemaNameKey(layout_node.name);
            const auto contract = mWidgetContracts.find(lookup);
            if (contract == mWidgetContracts.end())
            {
                state.result.error("view.element.unknown", "Unsupported XML element: " + layout_node.name + ".", current_source,
                                   layout_node.source.begin.line, layout_node.source.begin.column);
                return nullptr;
            }
            const std::string& tag = contract->second.element;
            if (contract->second.scoped_only)
            {
                state.result.error("view.element.scoped",
                                   "<" + tag + "> is valid only in its owning composite.",
                                   current_source, layout_node.source.begin.line, layout_node.source.begin.column);
                return nullptr;
            }

            loadWidgetDefaults(tag, state);
            const LayoutNode* defaults = state.widget_defaults.find(lookup)->second;
            const LayoutElement element(layout_node, defaults);
            std::string filename;
            if (tag != Panel::ELEMENT && readViewAttribute(element, "filename", filename))
            {
                state.result.error("view.filename.unsupported", "The filename attribute is only supported on <panel>.", current_source,
                                   element.source().begin.line, element.source().begin.column);
                return nullptr;
            }

            if (tag == Panel::ELEMENT && readViewAttribute(element, "filename", filename))
            {
                const bool rooted = filename.rfind("xui/", 0) == 0 || (!filename.empty() && (filename.front() == '/' || filename.front() == '\\'));
                const std::string resource = normalizeResource(rooted ? filename : directoryOf(current_source) + filename);
                if (resource.empty())
                {
                    state.result.error("view.panel.path_invalid", "Invalid panel filename: " + filename + ".", current_source,
                                       element.source().begin.line, element.source().begin.column);
                    return nullptr;
                }
                node = createResourceWidget(resource, state);
                if (!node) return nullptr;
                if (node->element() != Panel::ELEMENT)
                {
                    state.result.error("view.panel.root_invalid", "Referenced panel must have a <panel> root: " + resource + ".", current_source,
                                       element.source().begin.line, element.source().begin.column);
                    return nullptr;
                }
            }
            else if (!node)
            {
                node = contract->second.create();
            }

            Widget* target = node.get();
            if (!target || target->element() != tag)
            {
                state.result.error("view.builder.type_mismatch", "Registered builder does not create <" + tag + ">.", current_source,
                                   element.source().begin.line, element.source().begin.column);
                return nullptr;
            }

            validateViewAttributes(element, contract->second.attributes, state.result, current_source);
            applyCommonViewAttributes(element, *target, state.result, current_source, contract->second.supported_actions);
            if (contract->second.apply_attributes)
                contract->second.apply_attributes(element, *target, state.result, current_source, state.context);
            if (contract->second.validate_composition)
                state.authored_widgets.emplace(target, BuildState::AuthoredWidget{
                    target, &layout_node, defaults, &contract->second, current_source});

            if (contract->second.text_content == ViewTextContent::Inline)
            {
                if (contract->second.apply_inline_content)
                    contract->second.apply_inline_content(
                        compileInlineContent(element.content(), tag, contract->second.accepted_inline_content,
                                             state.result, current_source, state.context),
                        *target);
                return node;
            }

            std::string widget_text;
            bool pending_flow_break = false;
            bool has_layout_child = false;
            const auto markLayoutChild = [&](Widget& child)
            {
                detail::WidgetCompilerAccess::setFlowBreakBefore(child, pending_flow_break);
                pending_flow_break = false;
                has_layout_child = true;
            };
            for (const LayoutContent& content : element.content())
            {
                if (content.isText())
                {
                    std::string value = textKey(content.text);
                    if (value.empty()) continue;
                    if (contract->second.text_content == ViewTextContent::Unsupported)
                    {
                        state.result.error("view.text.unsupported", "Text content is not supported in <" + tag + ">.",
                                           current_source, content.source.begin.line, content.source.begin.column);
                    }
                    else if (contract->second.text_content == ViewTextContent::Widget)
                    {
                        if (!widget_text.empty()) widget_text += value;
                        else widget_text = std::move(value);
                    }
                    else
                    {
                        TextValue text = localizedViewText(std::move(value), state.result, current_source, state.context,
                                                           content.source.begin.line);
                        if (contract->second.create_text_child)
                        {
                            if (auto child = contract->second.create_text_child(std::move(text)))
                            {
                                markLayoutChild(*child);
                                target->addChild(std::move(child));
                            }
                        }
                        else
                        {
                            auto label = std::make_unique<Label>();
                            label->setText(std::move(text));
                            markLayoutChild(*label);
                            target->addChild(std::move(label));
                        }
                    }
                    continue;
                }

                const LayoutNode& child_node = *content.element;
                const LayoutElement child(child_node);
                if (schemaNameKey(child.name()) == schemaNameKey("br"))
                {
                    rejectLogicalAttributes(child, state.result, current_source);
                    if (hasAuthoredContent(child_node))
                        state.result.error("view.flow_break.children_unsupported",
                                           "Flow Break <br> cannot contain content.", current_source,
                                           child_node.source.begin.line, child_node.source.begin.column);
                    if (!has_layout_child)
                        state.result.error("view.flow_break.leading",
                                           "Flow Break requires a preceding layout child.", current_source,
                                           child_node.source.begin.line, child_node.source.begin.column);
                    else if (pending_flow_break)
                        state.result.error("view.flow_break.consecutive",
                                           "Consecutive Flow Break directives are not supported.", current_source,
                                           child_node.source.begin.line, child_node.source.begin.column);
                    else pending_flow_break = true;
                    continue;
                }

                const auto scoped_inline = contract->second.scoped_inline_content.find(schemaNameKey(child.name()));
                if (scoped_inline != contract->second.scoped_inline_content.end())
                {
                    rejectLogicalAttributes(child, state.result, current_source);
                    Widget* scoped_part = scoped_inline->second.apply(
                        compileInlineContent(child.content(), scoped_inline->second.element,
                                             scoped_inline->second.accepted, state.result,
                                             current_source, state.context),
                        *target, state.result, current_source,
                        child.source().begin.line, child.source().begin.column);
                    if (scoped_part) markLayoutChild(*scoped_part);
                    continue;
                }

                const auto part_contract = contract->second.part_attributes.find(schemaNameKey(child.name()));
                if (part_contract != contract->second.part_attributes.end())
                    validateViewAttributes(child, part_contract->second, state.result, current_source);
                if (contract->second.child_container)
                {
                    const std::optional<Widget*> container = contract->second.child_container(child, *target, state.result, current_source);
                    if (container)
                    {
                        if (*container)
                        {
                            for (const LayoutContent& nested_content : child.content())
                            {
                                if (nested_content.isText())
                                {
                                    if (!textKey(nested_content.text).empty())
                                        state.result.error("view.text.unsupported", "Text content is not supported in <" + child.name() + ">.",
                                                           current_source, nested_content.source.begin.line, nested_content.source.begin.column);
                                    continue;
                                }
                                if (auto child_widget = buildNode(*nested_content.element, current_source, nullptr))
                                    (*container)->addChild(std::move(child_widget));
                            }
                        }
                        continue;
                    }
                }
                if (auto child_widget = buildNode(child_node, current_source, nullptr))
                {
                    markLayoutChild(*child_widget);
                    target->addChild(std::move(child_widget));
                }
            }
            if (pending_flow_break)
                state.result.error("view.flow_break.trailing",
                                   "Flow Break requires a following layout child.", current_source,
                                   element.source().end.line, element.source().end.column);
            if (contract->second.text_content == ViewTextContent::Widget && !widget_text.empty() && contract->second.apply_text)
                contract->second.apply_text(std::move(widget_text), *target, state.result, current_source, state.context,
                                            element.source().begin.line);
            return node;
        };

        return buildNode(*document.root, document.source, std::move(root));
    }
}
