#include "linden_common.h"
#include "../test/lltut.h"
#include "rduipath.h"

namespace tut
{
    struct rduipath_data {};
    typedef test_group<rduipath_data> rduipath_test;
    typedef rduipath_test::object rduipath_object;
    rduipath_test rduipath_testcase("rduipath");

    template<> template<>
    void rduipath_object::test<1>()
    {
        rdui::PathCompileResult compiled = rdui::compileSvgPathData("M 0 0 L 10 0 h 5 v 10 l -5 0 Z");
        ensure("valid path compiles", compiled.ok());
        const rdui::Path& path = *compiled.path;
        ensure_equals("path command count", path.commands().size(), 6U);
        const auto contours = path.flatten();
        ensure_equals("one closed contour", contours.size(), 1U);
        ensure("closed contour repeats start", contours[0].front().x == contours[0].back().x);
    }

    template<> template<>
    void rduipath_object::test<2>()
    {
        rdui::Path curve;
        curve.moveTo(0.f, 0.f).cubicTo(0.f, 20.f, 20.f, 20.f, 20.f, 0.f);
        ensure("adaptive flatten emits curve segments", curve.flatten(0.25f).front().size() > 4U);
        ensure("looser tolerance emits fewer points", curve.flatten(4.f).front().size() < curve.flatten(0.1f).front().size());
    }

    template<> template<>
    void rduipath_object::test<3>()
    {
        const auto contour = rdui::Path::circle({11.f, 11.f}, 8.f).flatten().front();
        ensure("circle is closed", contour.front().x == contour.back().x && contour.front().y == contour.back().y);
        ensure("circle has useful resolution", contour.size() >= 24U);
    }

    template<> template<>
    void rduipath_object::test<4>()
    {
        const rdui::PathCompileResult compiled = rdui::compileSvgPathData("M0 0 L10", "broken.svg", 7);
        ensure("malformed path rejected", !compiled.ok());
        ensure("partial path never exposed", !compiled.path.has_value());
        ensure_equals("diagnostic source retained", compiled.errors.front().source, "broken.svg");
        ensure_equals("diagnostic line retained", compiled.errors.front().line, 7U);
    }

    template<> template<>
    void rduipath_object::test<5>()
    {
        const rdui::PathCompileResult compiled = rdui::compileSvgPathData("M0 0 A4 4 0 0 1 8 8");
        ensure("unsupported command rejected", !compiled.ok());
        ensure_equals("unsupported command diagnostic", compiled.errors.front().code, "svg.path.command_unsupported");
    }
}
