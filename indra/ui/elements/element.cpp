/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "elements/element.h"
#include <algorithm>
#include "elements/elementinternal.h"
#include "elements/elementtext.h"
#include "layout/engine.h"
#include "layout/resourcecompiler.h"
#include "render/paintcontext.h"
#include "style/style.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
Node* Node::nextSibling() noexcept {
    Node* parent = parentNode();
    if (!parent) return nullptr;
    const NodeList siblings = parent->childNodes();
    const auto found = std::find(siblings.begin(), siblings.end(), this);
    if (found == siblings.end()) return nullptr;
    const auto next = std::next(found);
    return next == siblings.end() ? nullptr : *next;
}

const Node* Node::nextSibling() const noexcept {
    const Node* parent = parentNode();
    if (!parent) return nullptr;
    const ConstNodeList siblings = parent->childNodes();
    const auto found = std::find(siblings.begin(), siblings.end(), this);
    if (found == siblings.end()) return nullptr;
    const auto next = std::next(found);
    return next == siblings.end() ? nullptr : *next;
}

Node* Node::before(NodePtr node) {
    Element* parent = parentNode() ? parentNode()->asElement() : nullptr;
    llassert_always(parent);
    return parent->insertBefore(std::move(node), this);
}

Node* Node::after(NodePtr node) {
    Element* parent = parentNode() ? parentNode()->asElement() : nullptr;
    llassert_always(parent);
    return parent->insertBefore(std::move(node), nextSibling());
}

NodePtr Node::replaceWith(NodePtr node) {
    Element* parent = parentNode() ? parentNode()->asElement() : nullptr;
    llassert_always(parent);
    return parent->replaceNode(*this, std::move(node));
}

NodePtr Node::remove() {
    Element* parent = parentNode() ? parentNode()->asElement() : nullptr;
    llassert_always(parent);
    return parent->removeNode(*this);
}

namespace detail {
namespace {
void appendPlainText(const Node& node, std::string& result) {
    if (const Text* text = node.asText()) {
        result += text->getData();
        return;
    }
    const Element* element = node.asElement();
    if (!element) return;
    if (element->elementName() == "br") {
        result += '\n';
        return;
    }
    bool first = true;
    for (const Node& child : nodes(*element)) {
        if (element->elementName() == "kbd" && !first && child.asElement()) result += ' ';
        appendPlainText(child, result);
        first = false;
    }
}
} // namespace

Element* findElementInScope(Element& element, std::string_view id) {
    if (element.id() == id) return &element;
    for (Element* child : element.children()) {
        if (child->id() == id) return child;
        if (child->idScopeRoot()) continue;
        if (Element* found = findElementInScope(*child, id)) return found;
    }
    return nullptr;
}

const Element* findElementInScope(const Element& element, std::string_view id) {
    if (element.id() == id) return &element;
    for (const Node& childNode : nodes(element)) {
        const Element* child = childNode.asElement();
        if (!child) continue;
        if (child->id() == id) return child;
        if (child->idScopeRoot()) continue;
        if (const Element* found = findElementInScope(*child, id)) return found;
    }
    return nullptr;
}

void indexElementsInScope(Element& element, std::map<std::string, Element*>& index) {
    if (!element.id().empty()) index.emplace(element.id(), &element);
    for (Element* child : element.children()) {
        if (child->idScopeRoot()) {
            if (!child->id().empty()) index.emplace(child->id(), child);
        } else {
            indexElementsInScope(*child, index);
        }
    }
}

const std::string* styleAttribute(const Element& element, std::string_view name) {
    const auto found = element.mStyleAttributes.find(std::string(name));
    return found == element.mStyleAttributes.end() ? nullptr : &found->second;
}

Node& appendText(Element& parent, std::string value) {
    if (!parent.mChildren.empty()) {
        if (Text* previous = parent.mChildren.back()->asText(); previous && !NodeAccess::flowBreakBefore(*previous)) {
            previous->setData(previous->getData() + value);
            return *previous;
        }
    }

    auto text = std::make_unique<Text>(std::move(value));
    Node* added = text.get();
    NodeAccess::setParent(*text, &parent);
    NodeAccess::setDocumentIdentity(*text, NodeAccess::documentIdentity(parent));
    parent.mChildren.push_back(std::unique_ptr<Node>(std::move(text)));
    ++parent.mChildSnapshotRevision;
    parent.invalidateMeasure();
    return *added;
}

Node& appendLocalizedText(Element& parent, LocalizedText text, std::string markup) {
    const std::size_t firstIndex = parent.mChildren.size();
    const bool previousSuppress = parent.mSuppressTextSlots;
    parent.mSuppressTextSlots = true;
    parent.rebuildResolvedMarkup(std::move(markup));
    parent.mSuppressTextSlots = previousSuppress;
    if (parent.mChildren.size() == firstIndex) appendText(parent, {});

    Node* first = parent.mChildren[firstIndex].get();
    Node* last = parent.mChildren.back().get();
    if (!parent.mSuppressTextSlots) parent.mTextContentSlots.push_back({std::move(text), first, last});
    return *first;
}

NodeChildren nodes(Element& element) {
    return NodeChildren(&element.mChildren);
}

ConstNodeChildren nodes(const Element& element) {
    return ConstNodeChildren(&element.mChildren);
}
} // namespace detail

Element::Element(const char* elementName)
    : Node(NodeType::Element), mElementName(elementName), mPrivate(std::make_unique<detail::ElementPrivateData>()) {}
Element::~Element() = default;

bool Element::flowBreakBefore() const {
    return detail::NodeAccess::flowBreakBefore(*this);
}

Node* Element::firstChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

const Node* Element::firstChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

NodeList Element::childNodes() {
    NodeList result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren) result.push_back(child.get());
    return result;
}

ConstNodeList Element::childNodes() const {
    ConstNodeList result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren) result.push_back(child.get());
    return result;
}

ElementList Element::children() {
    ElementList result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren)
        if (Element* element = child->asElement()) result.push_back(element);
    return result;
}

ConstElementList Element::children() const {
    ConstElementList result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren)
        if (const Element* element = child->asElement()) result.push_back(element);
    return result;
}

std::uint64_t Element::styleContextRevision() const {
    return mStyleRevision;
}

Element& Element::setId(std::string id) {
    mId = std::move(id);
    invalidateStyleTree();
    return *this;
}

Element& Element::addClass(std::string className) {
    mClasses.insert(std::move(className));
    invalidateStyleTree();
    return *this;
}

Element& Element::setStyleElement(std::string styleElement) {
    mStyleElement = std::move(styleElement);
    invalidateStyleTree();
    return *this;
}

Element& Element::setPart(std::string part) {
    mPart = std::move(part);
    invalidateStyleTree();
    return *this;
}

Element& Element::setRect(const Rect& rect) {
    const bool changed = !mRectExplicit || mRect.x != rect.x || mRect.y != rect.y || mRect.w != rect.w || mRect.h != rect.h;
    if (!changed) return *this;
    mRect = rect;
    mRectExplicit = true;
    invalidateMeasure();
    return *this;
}

void Element::scrollTo(float left, float top) {
    const float clampedLeft = std::clamp(left, 0.f, mScrollMetrics.maxScrollLeft);
    const float clampedTop = std::clamp(top, 0.f, mScrollMetrics.maxScrollTop);
    if (mScrollPosition.inlineOffset == clampedLeft && mScrollPosition.blockOffset == clampedTop) return;
    mScrollPosition.inlineOffset = clampedLeft;
    mScrollPosition.blockOffset = clampedTop;
    invalidatePaint();
    if (mSurface) mSurface->queueScrollNotification(*this);
}

void Element::scrollBy(float deltaLeft, float deltaTop) {
    scrollTo(mScrollPosition.inlineOffset + deltaLeft, mScrollPosition.blockOffset + deltaTop);
}

void Element::setScrollMetrics(const ScrollMetrics& metrics, const Rect& scrollableOverflow, const Rect& scrollport) {
    ScrollMetrics normalized = metrics;
    normalized.scrollWidth = std::max(0.f, normalized.scrollWidth);
    normalized.scrollHeight = std::max(0.f, normalized.scrollHeight);
    normalized.clientWidth = std::max(0.f, normalized.clientWidth);
    normalized.clientHeight = std::max(0.f, normalized.clientHeight);
    normalized.maxScrollLeft = std::clamp(normalized.maxScrollLeft, 0.f, std::max(0.f, normalized.scrollWidth - normalized.clientWidth));
    normalized.maxScrollTop = std::clamp(normalized.maxScrollTop, 0.f, std::max(0.f, normalized.scrollHeight - normalized.clientHeight));
    const float clampedLeft = std::clamp(mScrollPosition.inlineOffset, 0.f, normalized.maxScrollLeft);
    const float clampedTop = std::clamp(mScrollPosition.blockOffset, 0.f, normalized.maxScrollTop);
    const bool positionChanged = mScrollPosition.inlineOffset != clampedLeft || mScrollPosition.blockOffset != clampedTop;
    mScrollMetrics = normalized;
    mScrollPosition.inlineOffset = clampedLeft;
    mScrollPosition.blockOffset = clampedTop;
    mScrollableOverflow = scrollableOverflow;
    mScrollport = scrollport;
    if (positionChanged) {
        invalidatePaint();
        if (mSurface) mSurface->queueScrollNotification(*this);
    }
}

Element& Element::setPointerEvents(bool pointerEvents) {
    if (mPointerEvents == pointerEvents) return *this;
    mPointerEvents = pointerEvents;
    if (mSurface) mSurface->requestHitTestRefresh();
    return *this;
}

Element& Element::disabled(bool disabled) {
    const bool changed = disabled != this->disabled();
    setState(ElementState::Disabled, disabled);
    if (changed && mSurface) {
        mSurface->requestHitTestRefresh();
        if (disabled) mSurface->elementBecameUnavailable(*this);
    }
    return *this;
}

Element& Element::setVisibility(Visibility visibility) {
    if (mVisibilityOverride && *mVisibilityOverride == visibility) return *this;
    mVisibilityOverride = visibility;
    if (mSurface) mSurface->requestHitTestRefresh();
    invalidatePaint();
    if (visibility != Visibility::Visible && mSurface) mSurface->elementBecameUnavailable(*this);
    return *this;
}

Element& Element::setHidden(bool hidden) {
    return setVisibility(hidden ? Visibility::Hidden : Visibility::Visible);
}

namespace {
void validateChildAttachment(const Element& parent, const Node* child) {
    llassert_always(child);
    llassert_always(child->nodeType() != NodeType::Document);
    llassert_always(!child->parentNode());
    for (const Node* current = &parent; current; current = current->parentNode()) llassert_always(current != child);
    const auto& parentIdentity = detail::NodeAccess::documentIdentity(parent);
    const auto& childIdentity = detail::NodeAccess::documentIdentity(*child);
    const auto* parentIdentityToken = parentIdentity.get();
    const auto* childIdentityToken = childIdentity.get();
    if (parentIdentityToken) {
        if (childIdentityToken) llassert_always(childIdentityToken == parentIdentityToken);
    } else {
        llassert_always(!childIdentityToken);
    }
}

void assignDocumentIdentity(Node& node, const std::shared_ptr<detail::DocumentIdentity>& identity) {
    detail::NodeAccess::setDocumentIdentity(node, identity);
    if (Element* element = node.asElement())
        for (Node& child : detail::nodes(*element)) assignDocumentIdentity(child, identity);
}
} // namespace

Node* Element::append(NodePtr child) {
    validateChildAttachment(*this, child.get());
    Node* added = child.get();
    assignDocumentIdentity(*child, detail::NodeAccess::documentIdentity(*this));
    appendNode(std::move(child));
    return added;
}

Node* Element::prepend(NodePtr child) {
    validateChildAttachment(*this, child.get());
    Node* added = child.get();
    assignDocumentIdentity(*child, detail::NodeAccess::documentIdentity(*this));
    prependNode(std::move(child));
    return added;
}

void Element::replaceChildren() {
    replaceChildrenInternal();
}

Node* Element::replaceChildren(NodePtr child) {
    validateChildAttachment(*this, child.get());
    replaceChildrenInternal();
    return append(std::move(child));
}

Node* Element::insertBefore(NodePtr child, Node* reference) {
    validateChildAttachment(*this, child.get());
    auto position = reference ? std::find_if(mChildren.begin(), mChildren.end(), [reference](const NodePtr& node) { return node.get() == reference; })
                              : mChildren.end();
    llassert_always(!reference || position != mChildren.end());

    Node* added = child.get();
    assignDocumentIdentity(*child, detail::NodeAccess::documentIdentity(*this));
    detail::NodeAccess::setParent(*child, this);
    if (Element* element = child->asElement()) element->setSurface(mSurface);
    mChildren.insert(position, std::move(child));
    ++mChildSnapshotRevision;
    if (mSurface) mSurface->invalidateOrderingCache();
    if (Element* element = added->asElement()) {
        element->notifyTreeAttached();
        onChildAdded(*element);
    }
    invalidateMeasure();
    return added;
}

NodePtr Element::replaceNode(Node& child, NodePtr replacement) {
    validateChildAttachment(*this, replacement.get());
    auto found = std::find_if(mChildren.begin(), mChildren.end(), [&child](const NodePtr& node) { return node.get() == &child; });
    llassert_always(found != mChildren.end());

    Node* added = replacement.get();
    assignDocumentIdentity(*replacement, detail::NodeAccess::documentIdentity(*this));
    NodePtr detachedNode = std::move(*found);
    if (Element* oldElement = detachedNode->asElement()) oldElement->notifyTreeDetached();
    detail::NodeAccess::setParent(*detachedNode, nullptr);
    if (mSurface) {
        mSurface->invalidateOrderingCache();
        if (Element* oldElement = detachedNode->asElement()) {
            mSurface->elementBecameUnavailable(*oldElement);
            oldElement->setSurface(nullptr);
        }
    }
    detail::NodeAccess::setParent(*replacement, this);
    if (Element* element = replacement->asElement()) element->setSurface(mSurface);
    *found = std::move(replacement);
    mTextContentSlots.clear();
    ++mChildSnapshotRevision;
    if (Element* element = added->asElement()) {
        element->notifyTreeAttached();
        onChildAdded(*element);
    }
    invalidateMeasure();
    return detachedNode;
}

NodePtr Element::removeNode(Node& child) {
    auto found = std::find_if(mChildren.begin(), mChildren.end(), [&child](const NodePtr& node) { return node.get() == &child; });
    llassert_always(found != mChildren.end());
    NodePtr detachedNode = std::move(*found);
    mChildren.erase(found);
    if (Element* element = detachedNode->asElement()) element->notifyTreeDetached();
    detail::NodeAccess::setParent(*detachedNode, nullptr);
    if (mSurface) {
        mSurface->invalidateOrderingCache();
        if (Element* element = detachedNode->asElement()) {
            mSurface->elementBecameUnavailable(*element);
            element->setSurface(nullptr);
        }
    }
    mTextContentSlots.clear();
    ++mChildSnapshotRevision;
    invalidateMeasure();
    return detachedNode;
}

std::string Element::textContent() const {
    std::string result;
    detail::appendPlainText(*this, result);
    return result;
}

bool Element::isDisplayed(const Style& style) const {
    return style.display != DisplayMode::NoneValue && !mDisplayNoneOverride.value_or(false);
}

bool Element::isVisible(const Style& style) const {
    return isDisplayed(style) && mVisibilityOverride.value_or(style.visibility) == Visibility::Visible;
}

Element& Element::setDisplayNone(bool displayNone) {
    if (displayNone) {
        if (mDisplayNoneOverride && *mDisplayNoneOverride) return *this;
        mDisplayNoneOverride = true;
    } else {
        if (!mDisplayNoneOverride) return *this;
        mDisplayNoneOverride.reset();
    }
    ++mChildSnapshotRevision;
    if (mParent) ++mParent->mChildSnapshotRevision;
    if (mSurface) {
        mSurface->invalidateOrderingCache();
        mSurface->requestHitTestRefresh();
    }
    invalidateMeasure();
    if (displayNone && mSurface) mSurface->elementBecameUnavailable(*this);
    return *this;
}

Element& Element::textContent(std::string text) {
    mLocalizedContent.reset();
    clearDirectTextContent();
    detail::appendText(*this, std::move(text));
    return *this;
}

Element& Element::textContent(LocalizedText text) {
    mLocalizedContent = LocalizedContent{std::move(text), LocalizedContentMode::Literal};
    clearDirectTextContent();
    rebuildTextContent();
    return *this;
}

Element& Element::content(std::string text) {
    mLocalizedContent.reset();
    clearDirectTextContent();
    rebuildResolvedMarkup(std::move(text));
    return *this;
}

Element& Element::content(LocalizedText text) {
    mLocalizedContent = LocalizedContent{std::move(text), LocalizedContentMode::Markup};
    clearDirectTextContent();
    rebuildTextContent();
    return *this;
}

void Element::setKeybinding(std::string keybindingId) {
    mKeybindingId = std::move(keybindingId);
}

Element& Element::setOnActivate(std::function<void(Element&)> callback) {
    mOnActivate = std::move(callback);
    return *this;
}

Element& Element::setEventCall(std::string_view type, EventCall call) {
    mEventCalls.insert_or_assign(std::string(type), std::move(call));
    return *this;
}

void Element::addEventListener(std::string_view type, EventHandler handler, bool capture) {
    if (!handler) return;
    const auto duplicate = std::find_if(mEventListeners.begin(), mEventListeners.end(), [&](const EventListener& listener) {
        return listener.type == type && listener.capture == capture && listener.handler == handler;
    });
    if (duplicate != mEventListeners.end()) return;
    mEventListeners.push_back({std::string(type), std::move(handler), capture});
}

void Element::removeEventListener(std::string_view type, const EventHandler& handler, bool capture) {
    mEventListeners.erase(std::remove_if(mEventListeners.begin(), mEventListeners.end(),
                                         [&](const EventListener& listener) {
                                             return listener.type == type && listener.capture == capture && listener.handler == handler;
                                         }),
                          mEventListeners.end());
}

Element& Element::setIdScopeRoot(bool scopeRoot) {
    mIdScopeRoot = scopeRoot;
    return *this;
}

const EventCall* Element::eventCall(std::string_view type) const {
    const auto found = mEventCalls.find(type);
    return found == mEventCalls.end() ? nullptr : &found->second;
}

void Element::dispatchListeners(Event& event, bool capture) {
    const std::vector<EventListener> listeners = mEventListeners;
    for (const EventListener& listener : listeners) {
        if (event.immediatePropagationStopped()) break;
        if (listener.capture != capture || listener.type != event.type()) continue;
        listener.handler(event);
    }
}

void Element::dispatchEvent(Event& event) {
    if (mSurface) {
        mSurface->routeEvent(event);
        return;
    }
    event.setPhase(EventPhase::Target);
    event.setCurrentTarget(this);
    dispatchListeners(event, true);
    if (!event.immediatePropagationStopped()) dispatchListeners(event, false);
    event.setCurrentTarget(nullptr);
}

void Element::appendNode(NodePtr child) {
    Node* added = child.get();
    detail::NodeAccess::setParent(*child, this);
    if (Element* element = added->asElement()) element->setSurface(mSurface);
    mChildren.push_back(std::move(child));
    ++mChildSnapshotRevision;
    if (mSurface) mSurface->invalidateOrderingCache();
    if (Element* element = added->asElement()) {
        element->notifyTreeAttached();
        onChildAdded(*element);
    }
    invalidateMeasure();
}

void Element::prependNode(NodePtr child) {
    Node* added = child.get();
    detail::NodeAccess::setParent(*child, this);
    if (Element* element = added->asElement()) element->setSurface(mSurface);
    mChildren.insert(mChildren.begin(), std::move(child));
    ++mChildSnapshotRevision;
    if (mSurface) mSurface->invalidateOrderingCache();
    if (Element* element = added->asElement()) {
        element->notifyTreeAttached();
        onChildAdded(*element);
    }
    invalidateMeasure();
}

void Element::replaceChildrenInternal() {
    if (mSurface) mSurface->invalidateOrderingCache();
    mTextContentSlots.clear();
    Surface* surface = mSurface;
    std::vector<std::unique_ptr<Node>> children = std::move(mChildren);
    ++mChildSnapshotRevision;
    for (auto& childNode : children) {
        Element* child = childNode->asElement();
        if (child) child->notifyTreeDetached();
        detail::NodeAccess::setParent(*childNode, nullptr);
        if (!child) continue;
        child->setSurface(nullptr);
        if (surface) surface->elementBecameUnavailable(*child);
    }
    onChildrenCleared();
    invalidateMeasure();
}

void Element::clearDirectTextContent() {
    std::vector<std::unique_ptr<Node>> removed;
    for (auto current = mChildren.begin(); current != mChildren.end();) {
        Element* child = (*current)->asElement();
        if (child && !child->part().empty()) {
            ++current;
            continue;
        }
        removed.push_back(std::move(*current));
        current = mChildren.erase(current);
    }
    if (removed.empty()) return;

    mTextContentSlots.clear();
    ++mChildSnapshotRevision;
    if (mSurface) {
        mSurface->invalidateOrderingCache();
        for (const auto& node : removed)
            if (Element* child = node->asElement()) {
                child->notifyTreeDetached();
                detail::NodeAccess::setParent(*child, nullptr);
                child->setSurface(nullptr);
                mSurface->elementBecameUnavailable(*child);
            }
    } else {
        for (const auto& node : removed)
            if (Element* child = node->asElement()) {
                child->notifyTreeDetached();
                detail::NodeAccess::setParent(*child, nullptr);
                child->setSurface(nullptr);
            }
    }
    invalidateMeasure();
}

void Element::setSurface(Surface* surface) {
    if (mSurface == surface) return;
    if (mSurface) mSurface->invalidateStyleCache();
    detail::ElementInternalAccess::layoutCache(*this) = {};
    mSurface = surface;
    if (surface) {
        detail::ElementInternalAccess::mountLifetime(*this) =
            mParent ? detail::ElementInternalAccess::mountLifetime(*mParent) : std::make_shared<char>(0);
    } else {
        detail::ElementInternalAccess::mountLifetime(*this).reset();
    }
    const Element* expectedParent = mParent;
    const ElementRef<Element> self(this);
    std::vector<ElementRef<Element>> children;
    children.reserve(mChildren.size());
    for (const auto& childNode : mChildren)
        if (Element* child = childNode->asElement()) children.emplace_back(child);
    if (const System* system = this->system()) onLocaleChanged(*system);
    Element* current = self.get();
    if (!current || current->mSurface != surface || current->mParent != expectedParent) return;
    for (const ElementRef<Element>& childRef : children)
        if (Element* child = childRef.get(); child && child->parentElement() == current) {
            child->setSurface(surface);
            current = self.get();
            if (!current || current->mSurface != surface || current->mParent != expectedParent) return;
        }
    if (current->mSurface) {
        current->mSurface->invalidateStyleCache();
        current->mSurface->requestLayout();
    }
}

void Element::rebuildTextContent() {
    const bool previousBuilding = mBuildingTextContent;
    const bool previousSuppress = mSuppressTextSlots;
    mBuildingTextContent = true;
    mSuppressTextSlots = true;
    if (mLocalizedContent) {
        if (const System* currentSystem = system())
            if (mLocalizedContent->mode == LocalizedContentMode::Markup) rebuildResolvedMarkup(currentSystem->resolveMarkup(mLocalizedContent->text));
            else detail::appendText(*this, currentSystem->resolveMarkup(mLocalizedContent->text));
        else detail::appendText(*this, mLocalizedContent->text.key());
    }
    mSuppressTextSlots = previousSuppress;
    mBuildingTextContent = previousBuilding;
}

void Element::rebuildResolvedMarkup(std::string markup) {
    const std::string fallback = markup;
    LayoutBuildResult result = LayoutResourceCompiler().buildElementTreeFromString("<div>" + markup + "</div>", "localized-text.xml");
    if (result.hasErrors() || !result.document || !result.document->documentElement()) {
        detail::appendText(*this, fallback);
        return;
    }

    std::unique_ptr<Element> fragmentOwner = result.document->releaseDocumentElement();
    if (!fragmentOwner) {
        detail::appendText(*this, fallback);
        return;
    }
    Element& fragment = *fragmentOwner;
    if (fragment.mChildren.empty()) detail::appendText(fragment, {});
    for (auto& node : fragment.mChildren) {
        detail::NodeAccess::setParent(*node, this);
        if (Element* child = node->asElement()) {
            assignDocumentIdentity(*child, detail::NodeAccess::documentIdentity(*this));
            child->setSurface(mSurface);
        } else detail::NodeAccess::setDocumentIdentity(*node, detail::NodeAccess::documentIdentity(*this));
        Element* child = node->asElement();
        mChildren.push_back(std::move(node));
        if (child) {
            child->notifyTreeAttached();
            onChildAdded(*child);
        }
    }
    fragment.mChildren.clear();
    ++mChildSnapshotRevision;
    invalidateMeasure();
}

void Element::rebuildKeybindingContent(const System& system) {
    clearDirectTextContent();
    const KeybindingPresentation presentation = system.resolveKeybinding(mKeybindingId);
    for (const std::string& key : presentation.keys) {
        auto keyElement = std::make_unique<Element>("kbd");
        detail::appendText(*keyElement, key);
        append(std::move(keyElement));
    }
}

bool Element::refreshTextContentSlots() {
    if (mTextContentSlots.empty()) return false;

    for (TextContentSlot& slot : mTextContentSlots) {
        const auto first =
            std::find_if(mChildren.begin(), mChildren.end(), [&slot](const std::unique_ptr<Node>& node) { return node.get() == slot.first; });
        if (first == mChildren.end()) continue;
        const auto last = std::find_if(first, mChildren.end(), [&slot](const std::unique_ptr<Node>& node) { return node.get() == slot.last; });
        if (last == mChildren.end()) continue;

        const std::size_t insertionIndex = static_cast<std::size_t>(first - mChildren.begin());
        const bool flowBreakBefore = detail::NodeAccess::flowBreakBefore(*(*first));
        std::vector<std::unique_ptr<Node>> removed;
        for (auto current = first; current != last + 1; ++current) removed.push_back(std::move(*current));
        mChildren.erase(first, last + 1);
        for (auto& node : removed) {
            if (Element* child = node->asElement()) {
                child->notifyTreeDetached();
                detail::NodeAccess::setParent(*node, nullptr);
                child->setSurface(nullptr);
                if (mSurface) mSurface->elementBecameUnavailable(*child);
            } else {
                detail::NodeAccess::setParent(*node, nullptr);
            }
        }

        Element fragment("text-fragment");
        fragment.mSuppressTextSlots = true;
        if (const System* currentSystem = system()) fragment.rebuildResolvedMarkup(currentSystem->resolveMarkup(slot.text));
        else detail::appendText(fragment, slot.text.key());
        if (fragment.mChildren.empty()) detail::appendText(fragment, {});
        std::vector<std::unique_ptr<Node>> replacement = std::move(fragment.mChildren);
        detail::NodeAccess::setFlowBreakBefore(*replacement.front(), flowBreakBefore);
        Node* replacementFirst = replacement.front().get();
        Node* replacementLast = replacement.back().get();
        for (auto& node : replacement) {
            detail::NodeAccess::setParent(*node, this);
            if (Element* child = node->asElement()) {
                assignDocumentIdentity(*child, detail::NodeAccess::documentIdentity(*this));
                child->setSurface(mSurface);
            } else detail::NodeAccess::setDocumentIdentity(*node, detail::NodeAccess::documentIdentity(*this));
        }
        const std::size_t replacementCount = replacement.size();
        mChildren.insert(mChildren.begin() + static_cast<std::ptrdiff_t>(insertionIndex), std::make_move_iterator(replacement.begin()),
                         std::make_move_iterator(replacement.end()));
        for (std::size_t index = insertionIndex; index < insertionIndex + replacementCount; ++index)
            if (Element* child = mChildren[index]->asElement()) {
                child->notifyTreeAttached();
                onChildAdded(*child);
            }
        slot.first = replacementFirst;
        slot.last = replacementLast;
        ++mChildSnapshotRevision;
        if (mSurface) mSurface->invalidateOrderingCache();
        invalidateMeasure();
    }
    return true;
}

const System* Element::system() const {
    return mSurface ? mSurface->mSystem : nullptr;
}

const TextMetrics& Element::textMetrics() const {
    return mSurface ? mSurface->textMetrics() : fixedTextMetrics();
}

void Element::notifyTreeAttached() {
    onTreeAttached();
    const std::vector<ElementRef<Element>> children = [&] {
        std::vector<ElementRef<Element>> result;
        result.reserve(mChildren.size());
        for (const auto& childNode : mChildren)
            if (Element* child = childNode->asElement()) result.emplace_back(child);
        return result;
    }();
    for (const ElementRef<Element>& child : children)
        if (Element* current = child.get(); current && current->parentElement()) current->notifyTreeAttached();
}

void Element::notifyTreeDetached() {
    onTreeDetached();
    const std::vector<ElementRef<Element>> children = [&] {
        std::vector<ElementRef<Element>> result;
        result.reserve(mChildren.size());
        for (const auto& childNode : mChildren)
            if (Element* child = childNode->asElement()) result.emplace_back(child);
        return result;
    }();
    for (const ElementRef<Element>& child : children)
        if (Element* current = child.get(); current && current->parentElement()) current->notifyTreeDetached();
}

void Element::invalidateMeasure() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(kArrangeInvalidationReasons);
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Element::invalidateText() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(kTextInvalidationReasons);
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Element::invalidateArrange() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    if (mParent) mParent->invalidateArrange();
    else if (mSurface) mSurface->requestLayout();
}

void Element::invalidateArrangeTree() {
    const auto invalidate = [](auto&& self, Element& element) -> void {
        ++element.mLayoutInvalidationRevision;
        element.mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
        for (auto& childNode : element.mChildren)
            if (Element* child = childNode->asElement()) self(self, *child);
    };
    invalidate(invalidate, *this);
    if (mParent) mParent->invalidateArrange();
    else if (mSurface) mSurface->requestLayout();
}

void Element::invalidateTextTree() {
    const auto invalidate = [](auto&& self, Element& element) -> void {
        ++element.mLayoutInvalidationRevision;
        element.mInvalidationReasons.add(kTextInvalidationReasons);
        for (auto& childNode : element.mChildren)
            if (Element* child = childNode->asElement()) self(self, *child);
    };
    invalidate(invalidate, *this);
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Element::invalidateStyleTree(bool layoutAffecting, bool propagateToDescendants) {
    const auto invalidate = [layoutAffecting](auto&& self, Element& element, bool propagate) -> void {
        ++element.mStyleRevision;
        if (layoutAffecting) ++element.mLayoutInvalidationRevision;
        element.mInvalidationReasons.add(layoutAffecting ? kLayoutStyleInvalidationReasons : kPaintStyleInvalidationReasons);
        if (propagate)
            for (auto& childNode : element.mChildren)
                if (Element* child = childNode->asElement()) self(self, *child, true);
    };
    invalidate(invalidate, *this, propagateToDescendants);
    if (layoutAffecting) {
        ++mChildSnapshotRevision;
        if (mParent) ++mParent->mChildSnapshotRevision;
        if (mSurface) mSurface->invalidateOrderingCache();
    }
    if (!layoutAffecting) {
        if (mSurface) mSurface->requestPaint();
        return;
    }
    if (mParent) mParent->invalidateMeasure();
    else if (mSurface) mSurface->requestLayout();
}

void Element::invalidatePaint() {
    mInvalidationReasons.add(LayoutInvalidationReason::Paint);
    if (mSurface) mSurface->requestPaint();
}

void Element::clearPaintInvalidationTree() {
    mInvalidationReasons.remove(LayoutInvalidationReason::Paint);
    for (auto& childNode : mChildren)
        if (Element* child = childNode->asElement()) child->clearPaintInvalidationTree();
}

const StyleSheet* Element::styleSheet() const {
    return mSurface ? &mSurface->styleSheet() : nullptr;
}

void Element::setState(ElementState state, bool enabled) {
    if (radia::ui::hasState(mStates, state) == enabled) return;
    radia::ui::setState(mStates, state, enabled);
    const StyleSheet* styleSheet = this->styleSheet();
    const bool layoutAffecting = !styleSheet || styleSheet->stateAffectsLayout(*this, state);
    const bool propagateToDescendants = !styleSheet || styleSheet->stateAffectsDescendants(*this, state);
    invalidateStyleTree(layoutAffecting, propagateToDescendants);
    if (styleSheet && styleSheet->stateAffectsHitTesting(*this, state) && mSurface) mSurface->requestHitTestRefresh();
}

void Element::activate() {
    if (disabled()) return;
    Event event(kClickEvent, *this);
    dispatchEvent(event);
    if (event.defaultPrevented()) return;
    ElementRef<Element> self(this);
    onActivate();
    Element* current = self.get();
    if (!current) return;
    if (current->mOnActivate) current->mOnActivate(*current);
}

void Element::activateFromLabel() {
    const StyleSheet* styleSheet = this->styleSheet();
    for (const Element* current = this; current; current = current->parentElement()) {
        if (current->disabled()) return;
        if (styleSheet) {
            if (!current->isVisible(resolveElementStyle(*styleSheet, *current))) return;
        } else if (!current->isVisible(Style{})) {
            return;
        }
    }
    onLabelActivate();
}

void Element::translate(const Vec2& delta) {
    translateSubtree(delta);
    if (mSurface) mSurface->requestHitTestRefresh();
}

void Element::translateSubtree(const Vec2& delta) {
    mRect.x += delta.x;
    mRect.y += delta.y;
    mScrollableOverflow.x += delta.x;
    mScrollableOverflow.y += delta.y;
    mScrollport.x += delta.x;
    mScrollport.y += delta.y;
    for (auto& childNode : mChildren) {
        if (Element* child = childNode->asElement()) {
            child->translateSubtree(delta);
        } else if (Text* text = childNode->asText()) {
            Rect rect = text->rect();
            rect.x += delta.x;
            rect.y += delta.y;
            text->setRect(rect);
        }
    }
    mInvalidationReasons.add(LayoutInvalidationReason::Paint);
}

void Element::translateChild(Element& child, const Vec2& delta) {
    llassert_always(child.parentElement() == this);
    child.translate(delta);
}

Vec2 Element::intrinsicSize(const StyleSheet&, const Style&, const TextMetrics&, const IntrinsicSizeConstraints&) const {
    return {};
}

void Element::onLocaleChanged(const System& system) {
    if (mLocalizedContent) {
        clearDirectTextContent();
        rebuildTextContent();
    } else refreshTextContentSlots();
    if (!mKeybindingId.empty()) rebuildKeybindingContent(system);
}

bool Element::onKeybindingsChanged(const System& system) {
    bool changed = false;
    if (mLocalizedContent) {
        clearDirectTextContent();
        rebuildTextContent();
        changed = true;
    } else changed = refreshTextContentSlots();
    if (!mKeybindingId.empty()) {
        rebuildKeybindingContent(system);
        changed = true;
    }
    if (changed) invalidateText();
    return changed;
}

void Element::paint(PaintContext& context, const Style& style, float) const {
    context.paintBox(rect(), style);
}

bool Element::defaultKeyDown(const KeyEvent& event) {
    if (disabled() || !focusable() || !isActivationKey(event.key)) return false;
    setState(ElementState::Active, true);
    return true;
}

bool Element::defaultKeyUp(const KeyEvent& event) {
    if (disabled() || !focusable() || !isActivationKey(event.key)) return false;
    setState(ElementState::Active, false);
    activate();
    return true;
}

bool Element::defaultCharacterInput(unsigned int) {
    return false;
}

bool Element::defaultScroll(const WheelEvent&) {
    return false;
}

bool Element::beginPointerInteraction(const PointerEvent&) {
    return false;
}

bool Element::updatePointerInteraction(const PointerEvent&) {
    return false;
}

bool Element::endPointerInteraction(const PointerEvent&) {
    return false;
}
} // namespace radia::ui
