#include "linden_common.h"
#include "../test/lltut.h"

#include "rduifloaterresize.h"
#include <array>
#include <utility>

namespace tut
{
    struct rduifloaterresize_data {};
    typedef test_group<rduifloaterresize_data> rduifloaterresize_test;
    typedef rduifloaterresize_test::object rduifloaterresize_object;
    rduifloaterresize_test rduifloaterresize_testcase("rduifloaterresize");

    template<> template<>
    void rduifloaterresize_object::test<1>()
    {
        const rdui::Rect bounds{20.f, 30.f, 100.f, 80.f};
        using rdui::detail::ResizeEdges;
        const std::array<std::pair<rdui::Vec2, ResizeEdges>, 8> regions{{
            {{21.f, 70.f}, ResizeEdges::Left},
            {{119.f, 70.f}, ResizeEdges::Right},
            {{70.f, 31.f}, ResizeEdges::Bottom},
            {{70.f, 109.f}, ResizeEdges::Top},
            {{22.f, 32.f}, ResizeEdges::Left | ResizeEdges::Bottom},
            {{22.f, 108.f}, ResizeEdges::Left | ResizeEdges::Top},
            {{118.f, 32.f}, ResizeEdges::Right | ResizeEdges::Bottom},
            {{118.f, 108.f}, ResizeEdges::Right | ResizeEdges::Top},
        }};
        for (const auto& [point, expected] : regions)
            ensure("all edge and corner regions classify", rdui::detail::resizeEdgesAt(bounds, point) == expected);
        ensure("interior is not resizeable", rdui::detail::resizeEdgesAt(bounds, {70.f, 70.f}) == ResizeEdges::NoEdges);
        ensure("outside is not resizeable", rdui::detail::resizeEdgesAt(bounds, {19.f, 70.f}) == ResizeEdges::NoEdges);
    }

    template<> template<>
    void rduifloaterresize_object::test<2>()
    {
        using rdui::detail::ResizeEdges;
        const rdui::Rect initial{20.f, 30.f, 100.f, 80.f};
        const rdui::detail::FloaterResizeConstraints constraints{{40.f, 35.f}, rdui::Rect{0.f, 0.f, 200.f, 160.f}};

        const rdui::Rect left = rdui::detail::resizedRect(
            initial, {20.f, 70.f}, {115.f, 70.f}, ResizeEdges::Left, constraints);
        ensure_equals("left resize holds right edge", left.right(), initial.right());
        ensure_equals("left resize respects minimum", left.w, 40.f);

        const rdui::Rect corner = rdui::detail::resizedRect(
            initial, {120.f, 110.f}, {250.f, 250.f}, ResizeEdges::Right | ResizeEdges::Top, constraints);
        ensure_equals("right resize respects Surface", corner.right(), 200.f);
        ensure_equals("top resize respects Surface", corner.top(), 160.f);
        ensure_equals("corner holds left edge", corner.left(), initial.left());
        ensure_equals("corner holds bottom edge", corner.bottom(), initial.bottom());
    }

    template<> template<>
    void rduifloaterresize_object::test<3>()
    {
        using rdui::detail::ResizeEdges;
        ensure("left-bottom cursor", rdui::detail::resizeCursor(ResizeEdges::Left | ResizeEdges::Bottom)
            == rdui::CursorStyle::NortheastSouthwestResize);
        ensure("left-top cursor", rdui::detail::resizeCursor(ResizeEdges::Left | ResizeEdges::Top)
            == rdui::CursorStyle::NorthwestSoutheastResize);
        ensure("horizontal cursor", rdui::detail::resizeCursor(ResizeEdges::Right)
            == rdui::CursorStyle::EastWestResize);
        ensure("vertical cursor", rdui::detail::resizeCursor(ResizeEdges::Top)
            == rdui::CursorStyle::NorthSouthResize);
    }
}
