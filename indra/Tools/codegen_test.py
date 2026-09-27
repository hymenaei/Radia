#!/usr/bin/env python3
from __future__ import annotations

import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

import codegen


ROOT = Path(__file__).resolve().parents[2]


def _property(
    name: str,
    syntax: str | None = None,
    *,
    type_name: str | None = None,
    initial: str = "",
    inherited: bool = False,
    storage_path: tuple[str, ...] = (),
    storage_kind: str = "value",
    storage_name: str | None = None,
    group: tuple[str, str] | None = None,
    values: tuple[str, ...] = (),
    longhands: tuple[str, ...] = (),
    reset_longhands: tuple[str, ...] = (),
    syntax_parser: str | None = None,
) -> codegen.PropertyDefinition:
    return codegen.PropertyDefinition(
        name=name,
        syntax=codegen.parse_syntax(syntax) if syntax is not None else None,
        syntax_parser=syntax_parser,
        type_name=type_name,
        initial=initial,
        inherited=inherited,
        storage_path=storage_path,
        storage_kind=storage_kind,
        storage_name=storage_name,
        group=group,
        values=values,
        longhands=longhands,
        reset_longhands=reset_longhands,
        disables_native_appearance=False,
    )


def _catalogs(
    properties: tuple[codegen.PropertyDefinition, ...] = (),
    *,
    data_types: dict[str, str] | None = None,
    css_keywords: tuple[str, ...] = (),
    html_tags: dict[str, dict[str, object]] | None = None,
    html_attributes: tuple[str, ...] = (),
    input_types: tuple[str, ...] = (),
    events: dict[str, dict[str, object]] | None = None,
    pseudo_selectors: dict[str, dict[str, dict[str, object]]] | None = None,
) -> codegen.Catalogs:
    parsed_data_types = {
        name: codegen.parse_syntax(syntax) for name, syntax in (data_types or {}).items()
    }
    return codegen.Catalogs(
        properties=properties,
        data_types=parsed_data_types,
        css_keywords=css_keywords,
        html_tags=html_tags or {},
        html_attributes=html_attributes,
        input_types=input_types,
        events=events or {},
        pseudo_selectors=pseudo_selectors
        if pseudo_selectors is not None
        else {"pseudo-classes": {}, "pseudo-elements": {}},
    )


def _function_body(source: str, signature: str) -> str:
    start = source.index(signature)
    end = source.index("\n}", start) + 2
    return source[start:end]


def _enum_members(source: str, enum_name: str) -> list[str]:
    match = re.search(
        rf"enum class {re.escape(enum_name)}(?:\s*:\s*[\w:]+)?\s*\{{(.*?)\n\}};",
        source,
        re.DOTALL,
    )
    if match is None:
        raise AssertionError(f"missing {enum_name} declaration")
    return re.findall(r"^\s*([A-Za-z_]\w*)\s*,?$", match.group(1), re.MULTILINE)


def _descriptor_rows(source: str, enum_name: str, table_name: str) -> list[tuple[str, str]]:
    table = re.search(
        rf"inline constexpr \w+ {re.escape(table_name)}\[\] \{{(.*?)\n\}};",
        source,
        re.DOTALL,
    )
    if table is None:
        raise AssertionError(f"missing {table_name} descriptor table")
    rows = re.findall(
        rf'^\s*\{{\s*{re.escape(enum_name)}::([A-Za-z_]\w*)\s*,\s*("(?:\\.|[^"\\])*")',
        table.group(1),
        re.MULTILINE,
    )
    return [(member, json.loads(name)) for member, name in rows]


def _write_synthetic_catalogs(root: Path) -> None:
    directories = (
        root / "indra" / "Core" / "css" / "values",
        root / "indra" / "Core" / "html",
        root / "indra" / "Core" / "dom",
    )
    for directory in directories:
        directory.mkdir(parents=True, exist_ok=True)

    catalogs = {
        Path("indra/Core/css/CSSProperties.json5"): {
            "data_types": {},
            "properties": {
                "sample-value": {
                    "initial": "0",
                    "codegen": {
                        "syntax": "<number>",
                        "storage_path": ["mNonInheritedData", "sampleData"],
                        "type": "float",
                    },
                }
            },
        },
        Path("indra/Core/css/values/CSSKeywords.json5"): [*codegen.SYSTEM_COLORS, "auto"],
        Path("indra/Core/html/HTMLTags.json5"): {
            "x-sample": {"interface": "HTMLSampleElement"},
        },
        Path("indra/Core/html/HTMLAttributes.json5"): ["data-mode", "onready"],
        Path("indra/Core/html/InputTypes.json5"): ["text"],
        Path("indra/Core/dom/EventTypes.json5"): {"ready": {"interface": "Event"}},
        Path("indra/Core/css/CSSPseudoSelectors.json5"): {
            "pseudo-classes": {"focus": {}},
            "pseudo-elements": {"before": {"user_agent": True}},
        },
    }
    for relative_path, contents in catalogs.items():
        (root / relative_path).write_text(json.dumps(contents), encoding="utf-8")


class CodegenTests(unittest.TestCase):
    def test_storage_kinds_generate_matching_accessors(self) -> None:
        catalogs = _catalogs(
            (
                _property(
                    "sample-value",
                    "<number>",
                    type_name="float",
                    initial="2",
                    storage_path=("mData", "metrics"),
                ),
                _property(
                    "sample-reference",
                    "<number>",
                    type_name="SampleReference",
                    initial="0",
                    storage_path=("mData", "font"),
                    storage_kind="reference",
                ),
                _property(
                    "sample-mode",
                    "compact | roomy",
                    type_name="SampleMode",
                    initial="compact",
                    storage_path=("mData", "flags"),
                    storage_kind="enum",
                ),
                _property(
                    "sample-wrap",
                    "nowrap | wrap",
                    type_name="SampleRaw",
                    initial="nowrap",
                    storage_path=("mData", "flex"),
                    storage_kind="raw",
                ),
                _property(
                    "blend-mode",
                    "<number>",
                    type_name="float",
                    initial="1",
                    storage_path=("mData", "paint"),
                    storage_name="effectiveBlendMode",
                ),
            ),
            css_keywords=("compact", "roomy", "nowrap", "wrap"),
        )
        _, _, inline = codegen.generate_computed_style(catalogs)

        expected = (
            "float sampleValue() const {\n    return mData.metrics.sampleValue;",
            "const SampleReference& sampleReference() const {\n    return mData.font.sampleReference;",
            "if (value != mData.font.sampleReference)",
            "static_cast<SampleMode>(mData.flags.sampleMode)",
            "mData.flags.sampleMode = static_cast<unsigned>(value);",
            "SampleRaw::fromRaw(mData.flex.sampleWrap)",
            "mData.flex.sampleWrap = value.toRaw();",
            "return mData.paint.effectiveBlendMode;",
            "mData.paint.effectiveBlendMode = value;",
        )
        for fragment in expected:
            with self.subTest(fragment=fragment):
                self.assertIn(fragment, inline)

    def test_application_routes_value_initial_and_inherit(self) -> None:
        property = _property(
            "sample-value",
            "<number>",
            type_name="float",
            initial="3",
            inherited=True,
            storage_path=("mInheritedData", "metrics"),
        )
        header, source, inline = codegen.generate_computed_style(_catalogs((property,)))

        self.assertIn("static constexpr float initialSampleValue()", inline)
        self.assertIn("return 3.0f;", inline)
        self.assertIn(
            "static bool applyValueSampleValue(StyleBuilderState& builderState, const StyleValue& value)",
            source,
        )
        self.assertIn("auto parsedValue = toStyle<float>(builderState, value);", source)
        self.assertIn("if (!parsedValue) return false;", source)
        self.assertIn("builderState.style.setSampleValue(std::move(*parsedValue));", source)
        self.assertIn("static bool applyInitialSampleValue(StyleBuilderState& builderState)", source)
        self.assertIn("builderState.style.setSampleValue(ComputedStyle::initialSampleValue());", source)
        self.assertIn("static bool applyInheritSampleValue(StyleBuilderState& builderState)", source)
        self.assertIn("if (!builderState.parentStyle) return false;", source)
        self.assertIn("builderState.parentStyle->sampleValue()", source)
        self.assertIn("specifiedValue && applyValueSampleValue(builderState, *specifiedValue)", source)
        self.assertIn("case ApplyType::Initial: return applyInitialSampleValue(builderState);", source)
        self.assertIn("case ApplyType::Inherit: return applyInheritSampleValue(builderState);", source)
        property_case = source.split("case CSSProperty::SampleValue:", 1)[1].split(
            "        default: return false;", 1
        )[0]
        self.assertNotIn("default:", property_case)
        self.assertIn("case CSSProperty::SampleValue:\n            return true;", source)
        self.assertIn("enum class ApplyType : std::uint8_t { Value, Initial, Inherit };", header)

    def test_parser_combinators_generate_parser_structure(self) -> None:
        properties = tuple(
            _property(name, syntax, initial="0")
            for name, syntax in (
                ("alternative-value", "<number> | <percentage>"),
                ("any-order-value", "<number> || <percentage>"),
                ("all-order-value", "<number> && <percentage>"),
                ("sequence-value", "<number> / <percentage>"),
                ("function-value", "calc(<number>)"),
                ("required-value", "[ <number>? <percentage>? ]!"),
            )
        )
        _, source = codegen.generate_property_parsing(_catalogs(properties))
        cases = (
            ("alternative-value", "parseOneOf<"),
            ("any-order-value", "parseOneOrMoreAnyOrder<"),
            ("all-order-value", "parseAllAnyOrder<"),
            ("sequence-value", "parseSequence<"),
            ("function-value", 'parseFunction<"calc", consumeNumber<AnyRange>>'),
            ("required-value", "parseRequired<parseSequence<"),
        )
        for name, parser in cases:
            with self.subTest(property=name):
                body = _function_body(
                    source,
                    f"std::optional<CSSValue> parse{codegen._pascal_name(name)}(CSSValueRange& range) {{",
                )
                self.assertIn(parser, body)

        sequence = _function_body(
            source,
            "std::optional<CSSValue> parseSequenceValue(CSSValueRange& range) {",
        )
        self.assertIn("consumeLiteral<'/'>", sequence)
        self.assertIn("consumePercentage<AnyRange>", sequence)

    def test_repetition_bounds_generate_parser_calls(self) -> None:
        patterns = (
            ("zero-or-more", "<number>*", "parseStar<consumeNumber<AnyRange>>(range)"),
            ("one-or-more", "<number>+", "parsePlus<consumeNumber<AnyRange>>(range)"),
            ("optional-value", "<number>?", "parseOptional<consumeNumber<AnyRange>>(range)"),
            ("bounded-values", "<number>{1,3}", "parseRange<{1, 3}, consumeNumber<AnyRange>>(range)"),
            ("open-values", "<number>{2,}", "parseRange<{2, Infinite}, consumeNumber<AnyRange>>(range)"),
            ("comma-values", "<number>#", "parseHash<consumeNumber<AnyRange>>(range)"),
            ("two-comma-values", "<number>#{2}", "parseHash<2, consumeNumber<AnyRange>>(range)"),
            ("bounded-comma-values", "<number>#{2,4}", "parseHash<{2, 4}, consumeNumber<AnyRange>>(range)"),
        )
        properties = tuple(_property(name, syntax, initial="0") for name, syntax, _ in patterns)
        _, source = codegen.generate_property_parsing(_catalogs(properties))

        for name, _, expected in patterns:
            with self.subTest(property=name):
                body = _function_body(
                    source,
                    f"std::optional<CSSValue> parse{codegen._pascal_name(name)}(CSSValueRange& range) {{",
                )
                self.assertIn(expected, body)

    def test_syntax_round_trips_operators_and_groups(self) -> None:
        syntaxes = (
            "<length-percentage [0, inf]>{1,4} [ / <length-percentage [0, inf]>{1,4} ]?",
            "<sample-value>#",
            "<sample-value>#{2,4}",
            "<'first'> || <'second'>",
            "[ <sample-value>? ]!",
        )
        for syntax in syntaxes:
            with self.subTest(syntax=syntax):
                self.assertEqual(codegen.parse_syntax(syntax).render(), syntax)

    def test_property_references_and_data_types_resolve(self) -> None:
        catalogs = _catalogs(
            (
                _property("measure", "<number>", initial="0"),
                _property("scaled-measure", "<'measure'>", initial="0"),
                _property("ratio", "<ratio-value>", initial="0"),
            ),
            data_types={"<ratio-value>": "<number> | <percentage>"},
        )
        header, source = codegen.generate_property_parsing(catalogs)

        self.assertIn("std::optional<CSSValue> consumeRatioValue(CSSValueRange&);", header)
        self.assertIn("std::optional<CSSValue> parseMeasure(CSSValueRange&);", header)
        scaled_parser = _function_body(source, "std::optional<CSSValue> parseScaledMeasure")
        self.assertIn("return parseMeasure(range);", scaled_parser)
        self.assertIn("return consumeRatioValue(range);", _function_body(source, "std::optional<CSSValue> parseRatio"))
        self.assertIn("return parseOneOf<", _function_body(source, "std::optional<CSSValue> consumeRatioValue"))

        invalid = _catalogs((_property("bad-value", "<'missing-value'>"),))
        with self.assertRaisesRegex(codegen.GenerationError, "needs a generated longhand parser"):
            codegen.generate_property_parsing(invalid)

    def test_values_placeholder_expands_from_property_metadata(self) -> None:
        property = codegen._validate_property(
            "sample-mode",
            {
                "initial": "compact",
                "values": ["compact", "roomy"],
                "codegen": {
                    "syntax": "<<values>>",
                    "storage_path": ["mData"],
                    "storage_kind": "enum",
                    "type": "SampleMode",
                },
            },
            {"sample-mode"},
            Path("CSSProperties.json5"),
        )
        self.assertEqual(property.syntax.render(), "compact | roomy")
        self.assertEqual(property.values, ("compact", "roomy"))

        _, source = codegen.generate_property_parsing(
            _catalogs((property,), css_keywords=("compact", "roomy"))
        )
        self.assertIn(
            "consumeKeyword<\n        CSSKeyword::Compact,\n        CSSKeyword::Roomy>(range);",
            source,
        )

    def test_shorthand_patterns_are_inferred(self) -> None:
        cases = (
            ("CoalescingPair", ("north", "east"), "<'north'>{1,2}", "<length>"),
            ("CoalescingQuad", ("north", "east", "south", "west"), "<'north'>{1,4}", "<length>"),
            ("SpaceSeparated", ("first", "second"), "<'first'> <'second'>", None),
            ("SlashSeparated", ("first", "second"), "<'first'> / <'second'>", None),
            ("AnyOrder", ("first", "second"), "<'first'> || <'second'>", None),
            ("Layered", ("first", "second"), "[ <'first'> || <'second'> ]#", None),
        )
        for expected, names, syntax, shared_syntax in cases:
            with self.subTest(pattern=expected):
                components = tuple(
                    _property(name, shared_syntax or ("<number>" if index == 0 else "<percentage>"), initial="0")
                    for index, name in enumerate(names)
                )
                shorthand = _property("combined", syntax, longhands=names)
                header, source = codegen.generate_property_parsing(_catalogs((*components, shorthand)))
                self.assertIn(f"using ShorthandPattern = {expected};", header)
                self.assertIn(
                    "case CSSProperty::Combined: return parseShorthand<CSSProperty::Combined>(range, result);",
                    source,
                )

    def test_shorthand_traits_use_longhands_and_implemented_resets(self) -> None:
        longhands = (
            _property("first-part", "<number>", initial="0"),
            _property("second-part", "<number>", initial="1"),
        )
        reset = _property(
            "reset-part",
            "none",
            type_name="Filter",
            initial="none",
            storage_path=("mData", "filters"),
        )
        shorthand = _property(
            "combined",
            longhands=("first-part", "second-part"),
            reset_longhands=("reset-part",),
            syntax_parser="parseFlex",
        )
        properties = (*longhands, reset, shorthand)
        codegen._validate_shorthand(shorthand, {item.name: item for item in properties}, Path("CSSProperties.json5"))
        header, source = codegen.generate_property_parsing(_catalogs(properties, css_keywords=("none",)))

        shorthand_traits = header.split("PropertyTraits<CSSProperty::Combined>", 1)[1].split("};", 1)[0]
        self.assertIn("CSSProperty::FirstPart", shorthand_traits)
        self.assertIn("CSSProperty::SecondPart", shorthand_traits)
        self.assertIn("using ResetLonghands = PropertyList<", shorthand_traits)
        self.assertIn("CSSProperty::ResetPart", shorthand_traits)
        self.assertIn("using SyntaxParser = ParseFlex;", shorthand_traits)
        reset_traits = header.split("PropertyTraits<CSSProperty::ResetPart>", 1)[1].split("};", 1)[0]
        self.assertIn("initial = CSSKeyword::NoneValue;", reset_traits)
        self.assertNotIn("resetLonghands{{", header + source)

        invalid_reset = _property("reset-part", "none", initial="none")
        with self.assertRaisesRegex(codegen.GenerationError, "must be an implemented codegen longhand"):
            codegen._validate_shorthand(
                shorthand,
                {item.name: item for item in (*longhands, invalid_reset, shorthand)},
                Path("CSSProperties.json5"),
            )

    def test_edge_groups_generate_grouped_accessors(self) -> None:
        edge_names = ("north-edge", "east-edge", "south-edge", "west-edge")
        positions = ("top", "right", "bottom", "left")
        edges = tuple(
            _property(
                property_name,
                "<length>",
                type_name="EdgeLength",
                initial="0px",
                storage_path=("mData", "edges"),
                storage_kind="reference",
                group=("edges", position),
            )
            for property_name, position in zip(edge_names, positions, strict=True)
        )
        shorthand = _property("edges", "<'north-edge'>{1,4}", longhands=edge_names)
        properties = (*edges, shorthand)

        codegen._validate_property_groups({item.name: item for item in properties}, Path("CSSProperties.json5"))
        _, _, inline = codegen.generate_computed_style(_catalogs(properties))

        self.assertIn("RectEdges<EdgeLength> edges() const", inline)
        self.assertIn("return {northEdge(), eastEdge(), southEdge(), westEdge()};", inline)
        self.assertIn("void setEdges(RectEdges<EdgeLength> value)", inline)
        self.assertIn("void setEdges(EdgeLength value)", inline)
        self.assertIn("value.top", inline)
        self.assertIn("value.right", inline)
        self.assertIn("value.bottom", inline)
        self.assertIn("value.left", inline)

    def test_invalid_metadata_reports_the_conflict(self) -> None:
        cases = (
            (
                "syntax with custom parser",
                {
                    "codegen": {
                        "syntax": "<number>",
                        "syntax_parser": "parseFlex",
                        "storage_path": ["mData"],
                        "type": "float",
                    }
                },
                "syntax_parser cannot accompany syntax",
            ),
            (
                "custom parser without grammar documentation",
                {
                    "codegen": {
                        "syntax_parser": "parseFontFamily",
                        "storage_path": ["mData"],
                        "type": "SampleList",
                    }
                },
                "syntax_parser requires syntax_unused",
            ),
            (
                "unknown longhand",
                {
                    "initial": "0",
                    "longhands": ["missing-part"],
                    "codegen": {"syntax_parser": "parseFlex", "syntax_unused": "custom"},
                },
                "unknown longhands",
            ),
            (
                "duplicate longhand",
                {
                    "initial": "0",
                    "longhands": ["part", "part"],
                    "codegen": {"syntax_parser": "parseFlex", "syntax_unused": "custom"},
                },
                "duplicate longhand",
            ),
            (
                "unknown storage kind",
                {
                    "initial": "0",
                    "codegen": {
                        "syntax": "<number>",
                        "storage_path": ["mData"],
                        "storage_kind": "pointer",
                        "type": "float",
                    },
                },
                "unsupported value",
            ),
            (
                "missing direct storage",
                {
                    "initial": "0",
                    "codegen": {"syntax": "<number>", "type": "float"},
                },
                "storage_path: required",
            ),
            (
                "invalid storage identifier",
                {
                    "initial": "0",
                    "codegen": {
                        "syntax": "<number>",
                        "storage_path": ["mData"],
                        "storage_name": "bad-name",
                        "type": "float",
                    },
                },
                "not a C\\+\\+ identifier",
            ),
        )
        for case, metadata, message in cases:
            with self.subTest(case=case):
                with self.assertRaisesRegex(codegen.GenerationError, message):
                    codegen._validate_property("sample", metadata, {"sample", "part"}, Path("synthetic.json5"))

    def test_group_validation_rejects_incomplete_edges(self) -> None:
        grouped = tuple(
            _property(
                f"{position}-part",
                "<length>",
                type_name="EdgeLength",
                initial="0px",
                storage_path=("mData", "edges"),
                group=("edges", position),
            )
            for position in ("top", "right", "bottom")
        )
        shorthand = _property("edges", longhands=tuple(item.name for item in grouped), syntax_parser="parseFlex")
        with self.assertRaisesRegex(codegen.GenerationError, "all four edges or corners"):
            codegen._validate_property_groups(
                {item.name: item for item in (*grouped, shorthand)},
                Path("CSSProperties.json5"),
            )

    def test_cpp_names_and_strings_are_safe(self) -> None:
        self.assertEqual(codegen._pascal_name("sample-value"), "SampleValue")
        self.assertEqual(codegen._pascal_name("2d-axis"), "Value2dAxis")
        self.assertEqual(codegen._pascal_name("none"), "NoneValue")
        self.assertEqual(codegen._pascal_name("window"), "WindowValue")
        self.assertEqual(codegen._event_cpp_name("pointerdown"), "PointerDown")
        self.assertEqual(codegen._event_cpp_name("dblclick"), "DoubleClick")
        self.assertEqual(codegen._html_attribute_cpp_name("ondblclick"), "OnDoubleClick")

        value = 'quote " slash \\\\ newline\nand unicode é'
        self.assertEqual(json.loads(codegen._cpp_string(value)), value)

        with self.assertRaisesRegex(codegen.GenerationError, "collides"):
            codegen._validate_enum_names(("sample-value", "sampleValue"), "synthetic")
        with self.assertRaisesRegex(codegen.GenerationError, "Count enumerator"):
            codegen._validate_enum_names(("count",), "synthetic")

    def test_html_and_event_names_share_generated_spelling(self) -> None:
        catalogs = _catalogs(
            html_tags={"x-widget": {"interface": "HTMLWidgetElement", "void": True}},
            html_attributes=("data-mode", "ondblclick", "onpointerdown"),
            events={
                "dblclick": {"interface": "MouseEvent"},
                "pointerdown": {"interface": "PointerEvent"},
            },
        )
        html_header, _ = codegen.generate_html_names(catalogs)
        event_header, _ = codegen.generate_events(catalogs)

        self.assertIn('{ HTMLTag::XWidget, "x-widget", HTMLInterface::HTMLWidgetElement, true }', html_header)
        self.assertIn("OnDoubleClick,", html_header)
        self.assertIn("OnPointerDown,", html_header)
        self.assertIn('{ EventType::DoubleClick, "dblclick", HTMLAttribute::OnDoubleClick }', event_header)
        self.assertIn('{ EventType::PointerDown, "pointerdown", HTMLAttribute::OnPointerDown }', event_header)

    def test_json5_comments_and_duplicate_keys(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "catalog.json5"
            path.write_text("// header\n{ /* field */ value: 'ok', }\n", encoding="utf-8")
            self.assertEqual(codegen.load_json5(path), {"value": "ok"})

            path.write_text("{ item: {}, item: {}, }", encoding="utf-8")
            with self.assertRaisesRegex(codegen.GenerationError, "invalid JSON5"):
                codegen.load_json5(path)

    def test_generated_output_check_detects_drift(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            source_root = Path(directory) / "source"
            output_root = Path(directory) / "generated"
            _write_synthetic_catalogs(source_root)

            generated = codegen.generate(source_root, output_root)
            self.assertTrue(generated)
            codegen._check_output(source_root, output_root)

            stylesheet_output = output_root / "css" / "UserAgentStyleSheet.cpp"
            stylesheet_output.parent.mkdir()
            stylesheet_output.write_text("generated by CMake", encoding="utf-8")
            codegen._check_output(source_root, output_root)

            (output_root / "Unexpected.cpp").write_text("unexpected", encoding="utf-8")
            with self.assertRaisesRegex(codegen.GenerationError, "extra"):
                codegen._check_output(source_root, output_root)
            (output_root / "Unexpected.cpp").unlink()

            generated_file = output_root / "CSSKeywords.h"
            generated_file.write_text("stale", encoding="utf-8")
            with self.assertRaisesRegex(codegen.GenerationError, "changed"):
                codegen._check_output(source_root, output_root)

    def test_current_catalogs_load_and_generate_deterministically(self) -> None:
        catalogs = codegen.load_catalogs(ROOT)
        first = codegen.generate_outputs(catalogs)
        second = codegen.generate_outputs(catalogs)

        self.assertEqual(first, second)
        self.assertTrue(first)
        for name, contents in first.items():
            with self.subTest(output=name):
                self.assertTrue(contents.startswith("// Automatically generated from "))
                self.assertTrue(contents.endswith("\n"))

    def test_current_descriptor_tables_match_their_enums(self) -> None:
        outputs = codegen.generate_outputs(codegen.load_catalogs(ROOT))
        tables = (
            (
                "CSSProperties.h", "CSSProperty", "cssProperties",
                "CSSPropertyDescriptor", "cssPropertyDescriptor", "property",
            ),
            ("CSSKeywords.h", "CSSKeyword", "cssKeywords", "CSSKeywordDescriptor", "cssKeywordDescriptor", "keyword"),
            ("HTMLNames.h", "HTMLTag", "htmlTags", "HTMLTagDescriptor", "htmlTagDescriptor", "tag"),
            (
                "HTMLNames.h", "HTMLAttribute", "htmlAttributes",
                "HTMLAttributeDescriptor", "htmlAttributeDescriptor", "attribute",
            ),
            ("EventTypes.h", "EventType", "eventTypes", "EventTypeDescriptor", "eventTypeDescriptor", "type"),
            ("InputTypes.h", "InputType", "inputTypes", "InputTypeDescriptor", "inputTypeDescriptor", "type"),
            (
                "CSSPseudoSelectors.h", "CSSPseudoClass", "cssPseudoClasses",
                "CSSPseudoClassDescriptor", "cssPseudoClassDescriptor", "pseudoClass",
            ),
            (
                "CSSPseudoSelectors.h", "CSSPseudoElement", "cssPseudoElements",
                "CSSPseudoElementDescriptor", "cssPseudoElementDescriptor", "pseudoElement",
            ),
        )

        for output_name, enum_name, table_name, descriptor_type, accessor, parameter in tables:
            with self.subTest(table=table_name):
                header = outputs[output_name]
                enum_members = _enum_members(header, enum_name)
                if enum_members and enum_members[-1] == "Count":
                    enum_members.pop()
                rows = _descriptor_rows(header, enum_name, table_name)
                self.assertEqual([member for member, _ in rows], enum_members)
                names = [name for _, name in rows]
                self.assertEqual(len(names), len(set(names)))
                self.assertIn(
                    f"constexpr const {descriptor_type}* "
                    f"{accessor}({enum_name} {parameter}) {{",
                    header,
                )
                self.assertIn(f"return index < std::size({table_name}) ? &{table_name}[index] : nullptr;", header)


if __name__ == "__main__":
    unittest.main()
