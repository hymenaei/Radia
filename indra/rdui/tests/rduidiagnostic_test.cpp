#include "linden_common.h"
#include "../test/lltut.h"
#include "rduidiagnostic.h"

namespace tut
{
    struct rduidiagnostic_data {};
    typedef test_group<rduidiagnostic_data> rduidiagnostic_test;
    typedef rduidiagnostic_test::object rduidiagnostic_object;
    rduidiagnostic_test rduidiagnostic_testcase("rduidiagnostic");

    template<> template<>
    void rduidiagnostic_object::test<1>()
    {
        rdui::DiagnosticResult destination;
        destination.warning("warning.first", "first warning");
        destination.error("error.first", "first error");

        rdui::DiagnosticResult source;
        source.warning("warning.second", "second warning");
        source.warning("warning.third", "third warning");
        source.error("error.second", "second error");

        destination.append(std::move(source));

        ensure_equals("all warnings appended", destination.warnings.size(), std::size_t(3));
        ensure_equals("existing warning remains first", destination.warnings[0].code, std::string("warning.first"));
        ensure_equals("source warning order preserved", destination.warnings[1].code, std::string("warning.second"));
        ensure_equals("last source warning preserved", destination.warnings[2].code, std::string("warning.third"));
        ensure_equals("all errors appended", destination.errors.size(), std::size_t(2));
        ensure_equals("existing error remains first", destination.errors[0].code, std::string("error.first"));
        ensure_equals("source error follows", destination.errors[1].code, std::string("error.second"));
    }

    template<> template<>
    void rduidiagnostic_object::test<2>()
    {
        rdui::DiagnosticResult destination;
        rdui::DiagnosticResult empty;

        destination.append(std::move(empty));

        ensure("empty append has no errors", !destination.hasErrors());
        ensure("empty append has no warnings", destination.warnings.empty());
    }
}
