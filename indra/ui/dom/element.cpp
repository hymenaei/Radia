/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#include "linden_common.h"
#include "dom/element.h"
#include <algorithm>
#include "dom/elementinternal.h"
#include "dom/fragment.h"
#include "dom/fragmentinternal.h"
#include "dom/mutation.h"
#include "dom/text.h"
#include "event/eventcallinternal.h"
#include "html/elementnames.h"
#include "layout/engine.h"
#include "paint/paintcontext.h"
#include "style/computedstyle.h"
#include "style/pseudoelement.h"
#include "style/stylepass.h"
#include "surface/surface.h"
#include "system.h"
#include "text/metrics.h"

namespace radia::ui {
namespace {
bool isASCIIWhitespace(char character) {
    return character == '\t' || character == '\n' || character == '\f' || character == '\r' || character == ' ';
}

bool isValidClassToken(std::string_view className) {
    return !className.empty() && std::none_of(className.begin(), className.end(), isASCIIWhitespace);
}

std::string serializeClassTokens(const std::vector<std::string>& classes) {
    std::string result;
    for (const std::string& name : classes) {
        if (!result.empty()) result += ' ';
        result += name;
    }
    return result;
}
} // namespace

namespace detail {
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

Element* findElementInTree(Element& element, std::string_view id) {
    if (element.id() == id) return &element;
    for (Element* child : element.children())
        if (Element* found = findElementInTree(*child, id)) return found;
    return nullptr;
}

const Element* findElementInTree(const Element& element, std::string_view id) {
    if (element.id() == id) return &element;
    for (const Element* child : element.children())
        if (const Element* found = findElementInTree(*child, id)) return found;
    return nullptr;
}

void indexElementsInScope(Element& element, ElementIdIndex& index) {
    index.add(element);
    for (Element* child : element.children())
        if (child->idScopeRoot()) index.add(*child);
        else indexElementsInScope(*child, index);
}

void indexElementsInScope(const Element& element, ConstElementIdIndex& index) {
    index.add(element);
    for (const Element* child : element.children())
        if (child->idScopeRoot()) index.add(*child);
        else indexElementsInScope(*child, index);
}

Node& appendText(Element& parent, std::string value) {
    if (!parent.mChildren.empty()) {
        if (Text* previous = parent.mChildren.back()->asText(); previous && !NodeAccess::flowBreakBefore(*previous)) {
            previous->setData(previous->data() + value);
            return *previous;
        }
    }

    auto text = std::make_unique<Text>(std::move(value));
    Node* added = text.get();
    NodeMutation::insert(parent, std::move(text), nullptr);
    return *added;
}

Node& appendLocalizedText(Element& parent, LocalizedText text, std::string html) {
    const std::size_t firstIndex = parent.mChildren.size();
    const bool previousSuppress = parent.mSuppressTextSlots;
    parent.mSuppressTextSlots = true;
    parent.rebuildResolvedHTML(std::move(html));
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

std::weak_ptr<char> eventTargetLifetime(const Element& element) {
    return ElementInternalAccess::lifetime(element);
}
} // namespace detail

using detail::appendText;
using detail::ElementInternalAccess;
using detail::ElementPrivateData;
using detail::NodeAccess;
using detail::NodeMutation;
using detail::NodeRef;
using dom_detail::parseFragment;
using dom_detail::serializeChildren;

namespace {
FragmentPtr parseOrLiteralText(std::string html) {
    FragmentPtr fragment = parseFragment(html);
    if (!fragment) {
        fragment = std::make_unique<Fragment>();
        fragment->append(std::make_unique<Text>(std::move(html)));
    }
    return fragment;
}

FragmentPtr parseResolvedHTML(std::string html) {
    FragmentPtr fragment = parseOrLiteralText(std::move(html));
    if (fragment && !fragment->firstChild()) fragment->append(std::make_unique<Text>(std::string()));
    return fragment;
}

void replaceTextContent(Element& element, std::string text) {
    NodeMutation::replaceChildren(element, std::make_unique<Text>(std::move(text)));
}
} // namespace

Element::Element(std::string_view elementName)
    : Node(NodeType::Element), mElementName(elementName), mPrivate(std::make_unique<ElementPrivateData>()) {}
Element::~Element() {
    if (Surface* currentSurface = surface()) currentSurface->elementOwnerDestroyed(*this);
}

Element::ClassList Element::classList() {
    return ClassList(*this);
}

Element::ConstClassList Element::classList() const {
    return ConstClassList(*this);
}

Element::ClassList& Element::ClassList::add(std::string_view className) {
    if (!isValidClassToken(className)) return *this;
    std::string ownedClass(className);
    if (!mElement.mClasses.emplace(ownedClass).second) return *this;
    mElement.mClassOrder.push_back(std::move(ownedClass));
    mElement.setAttributeValue("class", serializeClassTokens(mElement.mClassOrder));
    mElement.invalidateStyleTree();
    mElement.invalidateFollowingSiblingStyleTrees(true, true);
    return *this;
}

Element::ClassList& Element::ClassList::remove(std::string_view className) {
    if (mElement.mClasses.erase(std::string(className)) == 0) return *this;
    mElement.mClassOrder.erase(std::remove(mElement.mClassOrder.begin(), mElement.mClassOrder.end(), className), mElement.mClassOrder.end());
    mElement.setAttributeValue("class", serializeClassTokens(mElement.mClassOrder));
    mElement.invalidateStyleTree();
    mElement.invalidateFollowingSiblingStyleTrees(true, true);
    return *this;
}

bool Element::ClassList::toggle(std::string_view className) {
    if (contains(className)) {
        remove(className);
        return false;
    }
    if (!isValidClassToken(className)) return false;
    add(className);
    return true;
}

bool Element::ClassList::replace(std::string_view oldClass, std::string_view newClass) {
    if (!isValidClassToken(oldClass) || !isValidClassToken(newClass)) return false;

    const auto oldSetEntry = mElement.mClasses.find(std::string(oldClass));
    if (oldSetEntry == mElement.mClasses.end()) return false;
    if (oldClass == newClass) return true;

    const auto oldOrderEntry = std::find(mElement.mClassOrder.begin(), mElement.mClassOrder.end(), oldClass);
    if (oldOrderEntry == mElement.mClassOrder.end()) return false;

    if (mElement.mClasses.find(std::string(newClass)) != mElement.mClasses.end()) {
        mElement.mClasses.erase(oldSetEntry);
        mElement.mClassOrder.erase(oldOrderEntry);
    } else {
        mElement.mClasses.erase(oldSetEntry);
        mElement.mClasses.emplace(std::string(newClass));
        *oldOrderEntry = std::string(newClass);
    }

    mElement.setAttributeValue("class", serializeClassTokens(mElement.mClassOrder));
    mElement.invalidateStyleTree();
    mElement.invalidateFollowingSiblingStyleTrees(true, true);
    return true;
}

bool Element::ClassList::contains(std::string_view className) const {
    return mElement.mClasses.find(std::string(className)) != mElement.mClasses.end();
}

bool Element::ConstClassList::contains(std::string_view className) const {
    return mElement.mClasses.find(std::string(className)) != mElement.mClasses.end();
}

void Element::setAttributeValue(std::string name, std::optional<std::string> value) {
    const auto found = std::find_if(mAttributes.begin(), mAttributes.end(), [&name](const Attribute& attribute) { return attribute.name == name; });
    if (found == mAttributes.end()) mAttributes.push_back({std::move(name), std::move(value)});
    else found->value = std::move(value);
}

void Element::removeAttributeValue(std::string_view name) {
    mAttributes.erase(std::remove_if(mAttributes.begin(), mAttributes.end(), [name](const Attribute& attribute) { return attribute.name == name; }),
                      mAttributes.end());
}

const Element::Attribute* Element::attribute(std::string_view name) const {
    const std::string canonicalName = canonicalizeHTMLName(name);
    const auto found = std::find_if(mAttributes.begin(), mAttributes.end(),
                                    [&canonicalName](const Attribute& attribute) { return attribute.name == canonicalName; });
    return found == mAttributes.end() ? nullptr : &*found;
}

bool Element::hasAttribute(std::string_view name) const {
    return attribute(name) != nullptr;
}

void Element::setAttribute(std::string name, std::optional<std::string> value) {
    name = canonicalizeHTMLName(name);
    if (name.empty()) {
        LL_WARNS("UI") << "Ignoring an empty HTML attribute name." << LL_ENDL;
        return;
    }
    if (name == "visibility") {
        LL_WARNS("UI") << "Ignoring unsupported HTML attribute: visibility." << LL_ENDL;
        return;
    }

    if (name == "id") {
        mId = value.value_or(std::string());
        setAttributeValue(std::move(name), std::move(value));
        invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        return;
    }
    if (name == "class") {
        mClasses.clear();
        mClassOrder.clear();
        if (value) {
            std::size_t begin = 0;
            while (begin < value->size()) {
                while (begin < value->size() && isASCIIWhitespace((*value)[begin])) ++begin;
                const std::size_t end = [&] {
                    std::size_t cursor = begin;
                    while (cursor < value->size() && !isASCIIWhitespace((*value)[cursor])) ++cursor;
                    return cursor;
                }();
                if (end > begin) {
                    std::string token = value->substr(begin, end - begin);
                    if (mClasses.emplace(token).second) mClassOrder.push_back(std::move(token));
                }
                begin = end;
            }
        }
        if (value) setAttributeValue(std::move(name), serializeClassTokens(mClassOrder));
        else setAttributeValue(std::move(name), std::nullopt);
        invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        return;
    }
    if (name == "disabled") {
        const bool changed = !disabled();
        setState(ElementState::Disabled, true);
        setAttributeValue(std::move(name), std::move(value));
        if (changed) {
            if (Surface* currentSurface = surface()) {
                currentSurface->requestHitTestRefresh();
                currentSurface->elementBecameUnavailable(*this);
            }
        }
        if (!changed) invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        return;
    }
    if (name == "hidden") {
        mVisibilityOverride = Visibility::Hidden;
        setAttributeValue(std::move(name), std::move(value));
        if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
        invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        invalidatePaint();
        if (Surface* currentSurface = surface()) currentSurface->elementBecameUnavailable(*this);
        return;
    }
    const std::string attributeName = name;
    const std::optional<std::string> authoredValue = value;
    setAttributeValue(std::move(name), std::move(value));
    invalidateStyleTree();
    invalidateFollowingSiblingStyleTrees(true, true);
    onAttributeSet(attributeName, authoredValue);
}

void Element::removeAttribute(std::string_view name) {
    const std::string canonicalName = canonicalizeHTMLName(name);
    if (canonicalName.empty()) {
        LL_WARNS("UI") << "Ignoring an empty HTML attribute name." << LL_ENDL;
        return;
    }
    name = canonicalName;
    if (name == "id") {
        mId.clear();
        removeAttributeValue(name);
        invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        return;
    }
    if (name == "class") {
        mClasses.clear();
        mClassOrder.clear();
        removeAttributeValue(name);
        invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        return;
    }
    if (name == "disabled") {
        const bool changed = disabled();
        setState(ElementState::Disabled, false);
        removeAttributeValue(name);
        if (changed) {
            if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
        }
        if (!changed) invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        return;
    }
    if (name == "hidden") {
        mVisibilityOverride = Visibility::Visible;
        removeAttributeValue(name);
        if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
        invalidateStyleTree();
        invalidateFollowingSiblingStyleTrees(true, true);
        invalidatePaint();
        return;
    }
    removeAttributeValue(name);
    invalidateStyleTree();
    invalidateFollowingSiblingStyleTrees(true, true);
    onAttributeRemoved(name);
}

bool Element::flowBreakBefore() const {
    return NodeAccess::flowBreakBefore(*this);
}

Node* Element::firstChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

const Node* Element::firstChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.front().get();
}

Node* Element::lastChild() noexcept {
    return mChildren.empty() ? nullptr : mChildren.back().get();
}

const Node* Element::lastChild() const noexcept {
    return mChildren.empty() ? nullptr : mChildren.back().get();
}

NodeSnapshot Element::childNodes() {
    NodeSnapshot result;
    result.reserve(mChildren.size());
    for (const auto& child : mChildren) result.push_back(child.get());
    return result;
}

ConstNodeSnapshot Element::childNodes() const {
    ConstNodeSnapshot result;
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

bool Element::disabled() const {
    if (radia::ui::hasState(mStates, ElementState::Disabled)) return true;

    for (const Element* ancestor = parentElement(); ancestor; ancestor = ancestor->parentElement()) {
        if (ancestor->elementName() != kFieldsetTag.localName || !radia::ui::hasState(ancestor->mStates, ElementState::Disabled)) continue;

        const Element* directChild = this;
        while (directChild && directChild->parentElement() != ancestor) directChild = directChild->parentElement();
        if (!directChild || directChild->elementName() != kLegendTag.localName) return true;

        const Element* firstLegend = nullptr;
        for (const Element* child : ancestor->children()) {
            if (child->elementName() == kLegendTag.localName) {
                firstLegend = child;
                break;
            }
        }
        if (directChild != firstLegend) return true;
    }
    return false;
}

std::uint64_t Element::styleContextRevision() const {
    return mStyleRevision;
}

Element& Element::setId(std::string id) {
    mId = std::move(id);
    if (mId.empty()) removeAttributeValue("id");
    else setAttributeValue("id", mId);
    invalidateStyleTree();
    invalidateFollowingSiblingStyleTrees(true, true);
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
    if (Surface* currentSurface = surface()) currentSurface->queueScrollNotification(*this);
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
        if (Surface* currentSurface = surface()) currentSurface->queueScrollNotification(*this);
    }
}

Element& Element::setPointerEvents(bool pointerEvents) {
    if (mPointerEvents == pointerEvents) return *this;
    mPointerEvents = pointerEvents;
    if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
    return *this;
}

Element& Element::disabled(bool disabled) {
    const bool changed = disabled != this->disabled();
    setState(ElementState::Disabled, disabled);
    if (disabled) setAttributeValue("disabled", std::nullopt);
    else removeAttributeValue("disabled");
    if (changed) {
        if (Surface* currentSurface = surface()) {
            currentSurface->requestHitTestRefresh();
            if (disabled) currentSurface->elementBecameUnavailable(*this);
        }
    }
    return *this;
}

Element& Element::setVisibility(Visibility visibility) {
    if (mVisibilityOverride && *mVisibilityOverride == visibility) return *this;
    mVisibilityOverride = visibility;
    removeAttributeValue("hidden");
    if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
    invalidateStyleTree();
    invalidatePaint();
    if (visibility != Visibility::Visible) {
        if (Surface* currentSurface = surface()) currentSurface->elementBecameUnavailable(*this);
    }
    return *this;
}

Element& Element::setHidden(bool hidden) {
    if (hidden) setAttribute("hidden");
    else removeAttribute("hidden");
    return *this;
}

Node* Element::append(NodePtr child) {
    return NodeMutation::insert(*this, std::move(child), nullptr);
}

Node* Element::append(FragmentPtr fragment) {
    return NodeMutation::insert(*this, std::move(fragment), nullptr);
}

Node* Element::prepend(NodePtr child) {
    return NodeMutation::insert(*this, std::move(child), firstChild());
}

Node* Element::prepend(FragmentPtr fragment) {
    return NodeMutation::insert(*this, std::move(fragment), firstChild());
}

void Element::replaceChildren() {
    NodeMutation::replaceChildren(*this);
}

void Element::replaceChildren(FragmentPtr fragment) {
    NodeMutation::replaceChildren(*this, std::move(fragment));
}

Node* Element::replaceChildren(NodePtr child) {
    return NodeMutation::replaceChildren(*this, std::move(child));
}

Node* Element::insertBefore(NodePtr child, Node* reference) {
    return NodeMutation::insert(*this, std::move(child), reference);
}

Node* Element::insertBefore(FragmentPtr fragment, Node* reference) {
    return NodeMutation::insert(*this, std::move(fragment), reference);
}

NodePtr Element::replaceNode(Node& child, NodePtr replacement) {
    return NodeMutation::replace(*this, child, std::move(replacement));
}

NodePtr Element::replaceNode(Node& child, FragmentPtr replacement) {
    return NodeMutation::replace(*this, child, std::move(replacement));
}

Node* Element::replaceRange(Node& first, Node& last, FragmentPtr replacement) {
    return NodeMutation::replaceRange(*this, first, last, std::move(replacement));
}

NodePtr Element::removeNode(Node& child) {
    return NodeMutation::remove(*this, child);
}

std::string Element::innerHTML() const {
    return serializeChildren(*this);
}

Element& Element::innerHTML(std::string html) {
    mLocalizedContent.reset();
    NodeMutation::replaceChildren(*this, parseOrLiteralText(std::move(html)));
    return *this;
}

std::string Element::textContent() const {
    std::string result;
    for (const Node* child : childNodes())
        if (const Text* text = child->asText()) result += text->data();
        else if (const Element* element = child->asElement()) result += element->textContent();
    return result;
}

bool Element::isDisplayed(const ComputedStyle& style) const {
    return style.display != DisplayMode::NoneValue && !mDisplayNoneOverride.value_or(false);
}

bool Element::isVisible(const ComputedStyle& style) const {
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
    if (Surface* currentSurface = surface()) {
        currentSurface->invalidateOrderingCache();
        currentSurface->requestHitTestRefresh();
    }
    invalidateMeasure();
    if (displayNone) {
        if (Surface* currentSurface = surface()) currentSurface->elementBecameUnavailable(*this);
    }
    return *this;
}

Element& Element::textContent(std::string text) {
    mLocalizedContent.reset();
    NodeMutation::replaceChildren(*this, std::make_unique<Text>(std::move(text)));
    return *this;
}

Element& Element::textContent(LocalizedText text) {
    mLocalizedContent = LocalizedContent{std::move(text), LocalizedContentMode::Literal};
    rebuildTextContent();
    return *this;
}

Element& Element::innerHTML(LocalizedText text) {
    mLocalizedContent = LocalizedContent{std::move(text), LocalizedContentMode::HTML};
    rebuildTextContent();
    return *this;
}

Element& Element::setOnActivate(std::function<void(Element&)> callback) {
    mOnActivate = std::move(callback);
    return *this;
}

void Element::addEventListener(std::string_view type, EventHandler handler, bool capture) {
    if (!handler) return;
    const auto duplicate = std::find_if(mEventListeners.begin(), mEventListeners.end(), [&](const EventListener& listener) {
        return listener.type == type && listener.capture == capture && listener.handler == handler;
    });
    if (duplicate != mEventListeners.end()) return;
    mEventListeners.push_back({std::string(type), std::move(handler), capture, std::make_shared<EventListener::State>()});
}

void Element::removeEventListener(std::string_view type, const EventHandler& handler, bool capture) {
    for (EventListener& listener : mEventListeners)
        if (listener.type == type && listener.capture == capture && listener.handler == handler && listener.state) listener.state->removed = true;
    mEventListeners.erase(std::remove_if(mEventListeners.begin(), mEventListeners.end(),
                                         [&](const EventListener& listener) {
                                             return listener.type == type && listener.capture == capture && listener.handler == handler;
                                         }),
                          mEventListeners.end());
}

Element& Element::setIdScopeRoot(bool scopeRoot) {
    if (mIdScopeRoot == scopeRoot) return *this;
    mIdScopeRoot = scopeRoot;
    ++mChildTopologyRevision;
    return *this;
}

void Element::dispatchListeners(Event& event, bool capture, const std::function<bool()>& isValid) {
    const EventListenerSnapshot listeners = mEventListeners;
    const ElementRef<Element> self(this);
    for (const EventListener& listener : listeners) {
        if (event.immediatePropagationStopped()) break;
        if (listener.capture != capture || listener.type != event.type() || (listener.state && listener.state->removed)) continue;
        listener.handler(event);
        if (!self || (isValid && !isValid())) {
            event.setCurrentTarget(nullptr);
            return;
        }
    }
}

void Element::dispatchEvent(Event& event) {
    if (!event.payloadMatchesType()) return;
    if (Surface* currentSurface = surface()) {
        currentSurface->routeEvent(event);
        return;
    }
    const ElementRef<Element> self(this);
    event.setPhase(EventPhase::Target);
    event.setCurrentTarget(this);
    dispatchListeners(event, true);
    if (!self) {
        event.setCurrentTarget(nullptr);
        return;
    }
    if (!event.immediatePropagationStopped()) dispatchListeners(event, false);
    event.setCurrentTarget(nullptr);
}

void Element::setSurface(Surface* surface) {
    if ((surface == nullptr && mSurface == nullptr) || (surface != nullptr && this->surface() == surface)) return;
    if (Surface* previousSurface = this->surface()) previousSurface->invalidateStyleCache();
    ElementInternalAccess::layoutCache(*this) = {};
    mSurface = surface;
    if (surface) mSurfaceLifetime = surface->mLifetime;
    else mSurfaceLifetime.reset();
    ++ElementInternalAccess::mountEpoch(*this).value;
    const Element* expectedParent = mParent;
    const ElementRef<Element> self(this);
    std::vector<ElementRef<Element>> children;
    children.reserve(mChildren.size());
    for (const auto& childNode : mChildren)
        if (Element* child = childNode->asElement()) children.emplace_back(child);
    if (const System* system = this->system()) onLocaleChanged(*system);
    Element* current = self.get();
    if (!current || current->surface() != surface || current->mParent != expectedParent) return;
    for (const ElementRef<Element>& childRef : children)
        if (Element* child = childRef.get(); child && child->parentElement() == current) {
            child->setSurface(surface);
            current = self.get();
            if (!current || current->surface() != surface || current->mParent != expectedParent) return;
        }
    if (Surface* currentSurface = current->surface()) {
        currentSurface->invalidateStyleCache();
        currentSurface->requestLayout();
    }
}

void Element::rebuildTextContent() {
    if (mLocalizedContent) {
        if (const System* currentSystem = system())
            if (mLocalizedContent->mode == LocalizedContentMode::HTML) replaceResolvedHTML(currentSystem->resolveHTML(mLocalizedContent->text));
            else replaceTextContent(*this, currentSystem->resolveHTML(mLocalizedContent->text));
        else replaceTextContent(*this, mLocalizedContent->text.key());
    }
}

void Element::rebuildResolvedHTML(std::string html) {
    append(parseResolvedHTML(std::move(html)));
}

void Element::replaceResolvedHTML(std::string html) {
    const ElementRef<Element> self(this);
    mTextContentSlots.clear();
    const bool previousSuppress = mSuppressTextSlots;
    mSuppressTextSlots = true;
    NodeMutation::replaceChildren(*this, parseResolvedHTML(std::move(html)));
    if (!self) return;
    mSuppressTextSlots = previousSuppress;
}

bool Element::refreshTextContentSlots() {
    if (mTextContentSlots.empty()) return false;
    const ElementRef<Element> self(this);

    for (TextContentSlot& slot : mTextContentSlots) {
        const auto first =
            std::find_if(mChildren.begin(), mChildren.end(), [&slot](const std::unique_ptr<Node>& node) { return node.get() == slot.first; });
        if (first == mChildren.end()) continue;
        const auto last = std::find_if(first, mChildren.end(), [&slot](const std::unique_ptr<Node>& node) { return node.get() == slot.last; });
        if (last == mChildren.end()) continue;

        const bool flowBreakBefore = NodeAccess::flowBreakBefore(*(*first));
        const std::string html = system() ? system()->resolveHTML(slot.text) : slot.text.key();
        FragmentPtr replacement = parseResolvedHTML(html);
        NodeRef replacementFirst(replacement->firstChild());
        NodeRef replacementLast(replacement->lastChild());
        NodeAccess::setFlowBreakBefore(*replacementFirst.get(), flowBreakBefore);
        const bool previousSuppress = mSuppressTextSlots;
        mSuppressTextSlots = true;
        replaceRange(*slot.first, *slot.last, std::move(replacement));
        if (!self) return true;
        mSuppressTextSlots = previousSuppress;
        slot.first = replacementFirst.get();
        slot.last = replacementLast.get();
    }
    return true;
}

const System* Element::system() const {
    const Surface* currentSurface = surface();
    return currentSurface ? currentSurface->mSystem : nullptr;
}

const TextMetrics& Element::textMetrics() const {
    const Surface* currentSurface = surface();
    return currentSurface ? currentSurface->textMetrics() : fixedTextMetrics();
}

void Element::notifyTreeAttached() {
    const ElementRef<Element> self(this);
    onTreeAttached();
    if (!self) return;
    const std::vector<ElementRef<Element>> children = [&] {
        std::vector<ElementRef<Element>> result;
        result.reserve(mChildren.size());
        for (const auto& childNode : mChildren)
            if (Element* child = childNode->asElement()) result.emplace_back(child);
        return result;
    }();
    for (const ElementRef<Element>& child : children)
        if (Element* current = child.get(); current && current->parentElement() == this) current->notifyTreeAttached();
}

void Element::notifyTreeWillBeDetached() {
    const ElementRef<Element> self(this);
    onTreeWillBeDetached();
    if (!self) return;
    const std::vector<ElementRef<Element>> children = [&] {
        std::vector<ElementRef<Element>> result;
        result.reserve(mChildren.size());
        for (const auto& childNode : mChildren)
            if (Element* child = childNode->asElement()) result.emplace_back(child);
        return result;
    }();
    for (const ElementRef<Element>& child : children)
        if (Element* current = child.get(); current && current->parentElement() == this) current->notifyTreeWillBeDetached();
}

void Element::notifyTreeDetached() {
    const ElementRef<Element> self(this);
    onTreeDetached();
    if (!self) return;
    const std::vector<ElementRef<Element>> children = [&] {
        std::vector<ElementRef<Element>> result;
        result.reserve(mChildren.size());
        for (const auto& childNode : mChildren)
            if (Element* child = childNode->asElement()) result.emplace_back(child);
        return result;
    }();
    for (const ElementRef<Element>& child : children)
        if (Element* current = child.get(); current && current->parentElement() == this) current->notifyTreeDetached();
}

void Element::invalidateMeasure() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(kArrangeInvalidationReasons);
    if (mParent) mParent->invalidateMeasure();
    else if (Surface* currentSurface = surface()) currentSurface->requestLayout();
}

void Element::invalidateText() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(kTextInvalidationReasons);
    if (mParent) mParent->invalidateMeasure();
    else if (Surface* currentSurface = surface()) currentSurface->requestLayout();
}

void Element::invalidateArrange() {
    ++mLayoutInvalidationRevision;
    mInvalidationReasons.add(LayoutInvalidationReason::Arrange);
    if (mParent) mParent->invalidateArrange();
    else if (Surface* currentSurface = surface()) currentSurface->requestLayout();
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
    else if (Surface* currentSurface = surface()) currentSurface->requestLayout();
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
    else if (Surface* currentSurface = surface()) currentSurface->requestLayout();
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
        if (Surface* currentSurface = surface()) currentSurface->invalidateOrderingCache();
    }
    if (!layoutAffecting) {
        if (Surface* currentSurface = surface()) currentSurface->requestPaint();
        return;
    }
    if (mParent) mParent->invalidateMeasure();
    else if (Surface* currentSurface = surface()) currentSurface->requestLayout();
}

void Element::invalidateFollowingSiblingStyleTrees(bool layoutAffecting, bool propagateToDescendants) {
    for (Node* sibling = nextSibling(); sibling; sibling = sibling->nextSibling())
        if (Element* element = sibling->asElement()) element->invalidateStyleTree(layoutAffecting, propagateToDescendants);
}

void Element::invalidateStyleTreesFrom(Node* firstChild, bool layoutAffecting, bool propagateToDescendants) {
    bool active = false;
    for (const NodePtr& childNode : mChildren) {
        if (!active && childNode.get() == firstChild) active = true;
        if (active)
            if (Element* child = childNode->asElement()) child->invalidateStyleTree(layoutAffecting, propagateToDescendants);
    }
}

void Element::invalidatePaint() {
    mInvalidationReasons.add(LayoutInvalidationReason::Paint);
    if (Surface* currentSurface = surface()) currentSurface->requestPaint();
}

void Element::clearPaintInvalidationTree() {
    mInvalidationReasons.remove(LayoutInvalidationReason::Paint);
    for (auto& childNode : mChildren)
        if (Element* child = childNode->asElement()) child->clearPaintInvalidationTree();
}

const StyleSheet* Element::styleSheet() const {
    if (Surface* currentSurface = surface()) return &currentSurface->styleSheet();
    return nullptr;
}

void Element::setState(ElementState state, bool enabled) {
    if (radia::ui::hasState(mStates, state) == enabled) return;
    const StyleSheet* styleSheet = this->styleSheet();
    const bool oldLayoutAffecting = styleSheet && styleSheet->stateAffectsLayout(*this, state);
    const bool oldDescendants = styleSheet && styleSheet->stateAffectsDescendants(*this, state);
    const bool oldFollowingSiblings = styleSheet && styleSheet->stateAffectsFollowingSiblings(*this, state);
    const bool oldHitTesting = styleSheet && styleSheet->stateAffectsHitTesting(*this, state);
    radia::ui::setState(mStates, state, enabled);
    const bool layoutAffecting = !styleSheet || oldLayoutAffecting || styleSheet->stateAffectsLayout(*this, state);
    const bool propagateToDescendants = !styleSheet || oldDescendants || styleSheet->stateAffectsDescendants(*this, state);
    invalidateStyleTree(layoutAffecting, propagateToDescendants);
    if (styleSheet && (oldFollowingSiblings || styleSheet->stateAffectsFollowingSiblings(*this, state)))
        invalidateFollowingSiblingStyleTrees(layoutAffecting, propagateToDescendants);
    if (styleSheet && (oldHitTesting || styleSheet->stateAffectsHitTesting(*this, state))) {
        if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
    }
}

void Element::activate() {
    if (disabled()) return;
    ElementRef<Element> self(this);
    Event event(kClickEvent, *this);
    dispatchEvent(event);
    Element* current = self.get();
    if (!current || event.defaultPrevented()) return;
    current->onActivate();
    current = self.get();
    if (!current) return;
    if (current->mOnActivate) current->mOnActivate(*current);
}

AccessibleSemantics Element::accessibleSemantics() const {
    AccessibleSemantics result;
    result.name = textContent();
    result.focusable = focusable();
    result.disabled = disabled();
    result.focused = hasState(ElementState::Focused);
    result.focusVisible = hasState(ElementState::FocusVisible);
    return result;
}

void Element::activateFromLabel() {
    const StyleSheet* styleSheet = this->styleSheet();
    std::optional<StylePass> styles;
    if (styleSheet) {
        const Surface* currentSurface = surface();
        styles.emplace(*styleSheet, textMetrics(), currentSurface ? currentSurface->layoutDirection() : LayoutDirection::LeftToRight);
    }
    for (const Element* current = this; current; current = current->parentElement()) {
        if (current->disabled()) return;
        if (!current->isVisible(styles ? styles->style(*current) : ComputedStyle{})) return;
    }
    if (Surface* currentSurface = surface()) currentSurface->setFocused(this, false);
    onLabelActivate();
}

void Element::translate(const Vec2& delta) {
    translateSubtree(delta);
    if (Surface* currentSurface = surface()) currentSurface->requestHitTestRefresh();
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
    for (PseudoElement* pseudoElement : generatedPseudoElements())
        if (pseudoElement) pseudoElement->translate(delta);
    mInvalidationReasons.add(LayoutInvalidationReason::Paint);
}

void Element::translateChild(Element& child, const Vec2& delta) {
    llassert_always(child.parentElement() == this);
    child.translate(delta);
}

Vec2 Element::intrinsicSize(const StyleSheet&, const ComputedStyle&, const TextMetrics&, const IntrinsicSizeConstraints&) const {
    return {};
}

void Element::onLocaleChanged(const System& system) {
    if (mLocalizedContent) rebuildTextContent();
    else refreshTextContentSlots();
}

void Element::paint(PaintContext& context, const ComputedStyle& style, float) const {
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
