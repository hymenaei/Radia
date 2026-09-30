#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import json5
import math
import re
import struct
import sys
import tempfile
from dataclasses import dataclass
from decimal import Decimal, DecimalException
from pathlib import Path
from typing import Any, Iterable


STORAGE_KINDS = frozenset({"value", "reference", "enum", "raw"})
SHORTHAND_SYNTAX_PARSERS = frozenset({"parseBorderImage", "parseBorderRadius", "parseFlex", "parseFont", "parsePlaceContent"})
CUSTOM_LONGHAND_PARSERS = frozenset({"parseBorderImageSource", "parseFontFamily"})
SYSTEM_COLORS = (
    "accentcolor",
    "accentcolortext",
    "activetext",
    "buttonborder",
    "buttonface",
    "buttontext",
    "canvas",
    "canvastext",
    "field",
    "fieldtext",
    "graytext",
    "highlight",
    "highlighttext",
    "linktext",
    "mark",
    "marktext",
    "selecteditem",
    "selecteditemtext",
    "visitedtext",
)
IDENTIFIER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*$")
PATH_MEMBER = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*$")
INITIAL_VALUE = re.compile(r"^([+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?)([A-Za-z]+|%)?$")
RANGE_BOUND = re.compile(r"^[+-]?(?:[0-9]+(?:\.[0-9]+)?|\.[0-9]+)(?:[eE][+-]?[0-9]+)?$")
CSS_INITIAL_UNITS = {
    "%": "Percentage",
    "px": "Px",
    "em": "Em",
    "rem": "Rem",
    "ch": "Ch",
    "ex": "Ex",
    "cap": "Cap",
    "ic": "Ic",
    "lh": "Lh",
    "rlh": "Rlh",
    "vw": "Vw",
    "vh": "Vh",
    "vmin": "Vmin",
    "vmax": "Vmax",
    "vi": "Vi",
    "vb": "Vb",
    "svw": "Svw",
    "svh": "Svh",
    "svmin": "Svmin",
    "svmax": "Svmax",
    "svi": "Svi",
    "svb": "Svb",
    "lvw": "Lvw",
    "lvh": "Lvh",
    "lvmin": "Lvmin",
    "lvmax": "Lvmax",
    "lvi": "Lvi",
    "lvb": "Lvb",
    "dvw": "Dvw",
    "dvh": "Dvh",
    "dvmin": "Dvmin",
    "dvmax": "Dvmax",
    "dvi": "Dvi",
    "dvb": "Dvb",
    "cm": "Cm",
    "mm": "Mm",
    "q": "Q",
    "in": "Inches",
    "pt": "Pt",
    "pc": "Pc",
    "deg": "Deg",
    "grad": "Grad",
    "rad": "Rad",
    "turn": "Turn",
    "s": "S",
    "ms": "Ms",
    "hz": "Hz",
    "khz": "KHz",
}
REF_STORAGE_MEMBERS = frozenset(
    {
        "mInheritedData",
        "mInheritedRareData",
        "mNonInheritedData",
        "fontData",
        "backgroundData",
        "surroundData",
        "miscData",
        "rareData",
        "flexibleBox",
    }
)


class GenerationError(ValueError):
    pass


def load_json5(path: Path) -> Any:
    try:
        return json5.loads(path.read_text(encoding="utf-8"), allow_duplicate_keys=False)
    except FileNotFoundError as error:
        raise GenerationError(f"missing catalog: {path}") from error
    except ValueError as error:
        raise GenerationError(f"{path}: invalid JSON5: {error}") from error


def _require_dict(value: Any, label: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise GenerationError(f"{label}: expected an object")
    return value


def _require_string(value: Any, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise GenerationError(f"{label}: expected a non-empty string")
    return value


def _require_string_list(value: Any, label: str) -> list[str]:
    if not isinstance(value, list) or any(not isinstance(item, str) or not item for item in value):
        raise GenerationError(f"{label}: expected a list of non-empty strings")
    return list(value)


def _name_map(value: Any, path: Path) -> dict[str, dict[str, Any]]:
    if isinstance(value, list):
        result: dict[str, dict[str, Any]] = {}
        for index, name in enumerate(value):
            if not isinstance(name, str) or not name:
                raise GenerationError(f"{path}[{index}]: expected a non-empty string")
            if name in result:
                raise GenerationError(f"{path}: duplicate name {name!r}")
            result[name] = {}
        return result

    data = _require_dict(value, str(path))
    result = {}
    for name, metadata in data.items():
        if not isinstance(name, str) or not name:
            raise GenerationError(f"{path}: catalog names must be non-empty strings")
        result[name] = _require_dict(metadata, f"{path}.{name}")
    return result


def _data_type_map(value: Any, path: Path) -> dict[str, SyntaxNode]:
    data = _require_dict(value, str(path))
    result: dict[str, SyntaxNode] = {}
    for name, syntax in data.items():
        if not isinstance(name, str) or len(name) < 3 or name[0] != "<" or name[-1] != ">":
            raise GenerationError(f"{path}: data type names must be enclosed in angle brackets")
        result[name] = parse_syntax(_require_string(syntax, f"{path}.{name}"))
    return result


def _cpp_identifier(value: str, label: str) -> None:
    if not IDENTIFIER.fullmatch(value):
        raise GenerationError(f"{label}: {value!r} is not a C++ identifier")


_CPP_ENUM_RESERVED_NAMES = {
    "Bool": "BoolValue",
    "Cursor": "CursorValue",
    "Display": "DisplayValue",
    "None": "NoneValue",
    "Status": "StatusValue",
    "Success": "SuccessValue",
    "Window": "WindowValue",
}

_CSS_KEYWORD_NAME_OVERRIDES = {
    "accentcolor": "AccentColor",
    "accentcolortext": "AccentColorText",
    "activetext": "ActiveText",
    "buttonborder": "ButtonBorder",
    "buttonface": "ButtonFace",
    "buttontext": "ButtonText",
    "canvas": "Canvas",
    "canvastext": "CanvasText",
    "field": "Field",
    "fieldtext": "FieldText",
    "graytext": "GrayText",
    "highlight": "Highlight",
    "highlighttext": "HighlightText",
    "linktext": "LinkText",
    "mark": "Mark",
    "marktext": "MarkText",
    "selecteditem": "SelectedItem",
    "selecteditemtext": "SelectedItemText",
    "visitedtext": "VisitedText",
    "currentcolor": "CurrentColor",
}


def _pascal_name(value: str, reserved: bool = True) -> str:
    parts = re.findall(r"[A-Za-z0-9]+", value)
    result = _CSS_KEYWORD_NAME_OVERRIDES.get(value, "".join(part[0].upper() + part[1:] for part in parts))
    if not result:
        result = "Value"
    if result[0].isdigit():
        result = "Value" + result
    return _CPP_ENUM_RESERVED_NAMES.get(result, result) if reserved else result


def _event_cpp_name(name: str) -> str:
    special_names = {
        "characterinput": "CharacterInput",
        "contextmenu": "ContextMenu",
        "dblclick": "DoubleClick",
    }
    if name in special_names:
        return special_names[name]
    for prefix in ("key", "mouse", "pointer"):
        suffix = name.removeprefix(prefix)
        if name.startswith(prefix) and suffix in {"down", "move", "up"}:
            return _pascal_name(prefix) + _pascal_name(suffix)
    return _pascal_name(name)


def _html_attribute_cpp_name(name: str) -> str:
    if name.startswith("on"):
        return "On" + _event_cpp_name(name[2:])
    return _pascal_name(name)


def _cpp_string(value: str) -> str:
    return json.dumps(value, ensure_ascii=False)


def _validate_enum_names(names: Iterable[str], label: str) -> None:
    generated: dict[str, str] = {}
    for name in names:
        identifier = _pascal_name(name)
        previous = generated.get(identifier)
        if previous is not None:
            raise GenerationError(f"{label}: {name!r} collides with {previous!r} as {identifier}")
        if identifier == "Count":
            raise GenerationError(f"{label}: {name!r} collides with the generated Count enumerator")
        generated[identifier] = name


@dataclass(frozen=True)
class SyntaxToken:
    kind: str
    text: str
    position: int


class SyntaxLexer:
    _single = frozenset("|?+#{}[](),/*!")

    def __init__(self, source: str) -> None:
        self.source = source

    def lex(self) -> list[SyntaxToken]:
        tokens: list[SyntaxToken] = []
        position = 0
        while position < len(self.source):
            if self.source[position].isspace():
                position += 1
                continue

            start = position
            if self.source.startswith("||", position) or self.source.startswith("&&", position):
                tokens.append(SyntaxToken(self.source[position : position + 2], self.source[position : position + 2], position))
                position += 2
                continue

            if self.source.startswith("<<", position):
                end = self.source.find(">>", position + 2)
                if end == -1:
                    raise GenerationError(f"syntax at {position}: unclosed type reference")
                end += 2
                tokens.append(SyntaxToken("type", self.source[position:end], position))
                position = end
                continue

            if self.source[position] == "<":
                end = self.source.find(">", position + 1)
                if end == -1:
                    raise GenerationError(f"syntax at {position}: unclosed type reference")
                end += 1
                tokens.append(SyntaxToken("type", self.source[position:end], position))
                position = end
                continue

            if self.source[position] in self._single:
                character = self.source[position]
                tokens.append(SyntaxToken(character, character, position))
                position += 1
                continue

            while position < len(self.source):
                if self.source[position].isspace() or self.source[position] in self._single or self.source[position] in "<>&":
                    break
                position += 1
            if position == start:
                raise GenerationError(f"syntax at {position}: unsupported character {self.source[position]!r}")
            word = self.source[start:position]
            if position < len(self.source) and self.source[position] == "(":
                tokens.append(SyntaxToken("function", word, start))
                position += 1
            else:
                tokens.append(SyntaxToken("word", word, start))

        tokens.append(SyntaxToken("eof", "", len(self.source)))
        return tokens


@dataclass(frozen=True)
class SyntaxNode:
    kind: str
    value: str = ""
    children: tuple["SyntaxNode", ...] = ()
    minimum: int | None = None
    maximum: int | None = None
    bounds: tuple[str, str] | None = None

    def render(self) -> str:
        if self.kind in {"keyword", "literal"}:
            result = self.value
        elif self.kind == "type":
            result = f"<{self.value}>"
            if self.bounds:
                result = f"<{self.value} [{self.bounds[0]}, {self.bounds[1]}]>"
        elif self.kind == "sequence":
            result = " ".join(child.render() for child in self.children)
        elif self.kind == "alternative":
            result = " | ".join(child.render() for child in self.children)
        elif self.kind == "any_order":
            result = " || ".join(child.render() for child in self.children)
        elif self.kind == "all_order":
            result = " && ".join(child.render() for child in self.children)
        elif self.kind == "group":
            result = f"[ {self.children[0].render()} ]"
        elif self.kind == "function":
            result = f"{self.value}({self.children[0].render()})"
        elif self.kind == "required":
            result = f"{self.children[0].render()}!"
        else:
            result = self.value

        if self.kind == "repeat":
            child = self.children[0]
            if self.minimum == 0 and self.maximum == 1:
                return f"{child.render()}?"
            if self.minimum == 1 and self.maximum is None:
                return f"{child.render()}#" if self.value == "comma" else f"{child.render()}+"
            if self.minimum == 0 and self.maximum is None:
                return f"{child.render()}*"
            if self.value == "comma":
                if self.minimum == self.maximum:
                    suffix = f"#{{{self.minimum}}}"
                else:
                    maximum = "" if self.maximum is None else str(self.maximum)
                    suffix = f"#{{{self.minimum},{maximum}}}"
            elif self.minimum == self.maximum:
                suffix = f"{{{self.minimum}}}"
            else:
                maximum = "" if self.maximum is None else str(self.maximum)
                suffix = f"{{{self.minimum},{maximum}}}"
            return f"{child.render()}{suffix}"
        return result


class SyntaxParser:
    def __init__(self, source: str) -> None:
        self.tokens = SyntaxLexer(source).lex()
        self.index = 0

    def parse(self) -> SyntaxNode:
        if self._peek().kind == "eof":
            raise GenerationError("syntax: expected an expression")
        result = self._parse_alternative()
        if self._peek().kind != "eof":
            token = self._peek()
            raise GenerationError(f"syntax at {token.position}: unexpected {token.text!r}")
        return result

    def _peek(self) -> SyntaxToken:
        return self.tokens[self.index]

    def _accept(self, kind: str) -> bool:
        if self._peek().kind != kind:
            return False
        self.index += 1
        return True

    def _expect(self, kind: str) -> SyntaxToken:
        token = self._peek()
        if token.kind != kind:
            raise GenerationError(f"syntax at {token.position}: expected {kind!r}, got {token.text!r}")
        self.index += 1
        return token

    def _parse_alternative(self) -> SyntaxNode:
        children = [self._parse_any_order()]
        while self._accept("|"):
            children.append(self._parse_any_order())
        return SyntaxNode("alternative", children=tuple(children)) if len(children) > 1 else children[0]

    def _parse_any_order(self) -> SyntaxNode:
        children = [self._parse_all_order()]
        while self._accept("||"):
            children.append(self._parse_all_order())
        return SyntaxNode("any_order", children=tuple(children)) if len(children) > 1 else children[0]

    def _parse_all_order(self) -> SyntaxNode:
        children = [self._parse_sequence()]
        while self._accept("&&"):
            children.append(self._parse_sequence())
        return SyntaxNode("all_order", children=tuple(children)) if len(children) > 1 else children[0]

    def _starts_primary(self) -> bool:
        return self._peek().kind not in {"eof", "|", "||", "&&", "]", ")", "}"}

    def _parse_sequence(self) -> SyntaxNode:
        children: list[SyntaxNode] = []
        while self._starts_primary():
            children.append(self._parse_postfix())
        if not children:
            token = self._peek()
            raise GenerationError(f"syntax at {token.position}: expected a value")
        return SyntaxNode("sequence", children=tuple(children)) if len(children) > 1 else children[0]

    def _parse_postfix(self) -> SyntaxNode:
        node = self._parse_primary()
        while True:
            if self._accept("?"):
                node = SyntaxNode("repeat", children=(node,), minimum=0, maximum=1)
            elif self._accept("+"):
                node = SyntaxNode("repeat", children=(node,), minimum=1, maximum=None)
            elif self._accept("*"):
                node = SyntaxNode("repeat", children=(node,), minimum=0, maximum=None)
            elif self._accept("#"):
                minimum, maximum = 1, None
                if self._accept("{"):
                    minimum = self._number("repeat minimum")
                    if self._accept(","):
                        maximum = None if self._peek().kind == "}" else self._number("repeat maximum")
                    else:
                        maximum = minimum
                    self._expect("}")
                node = SyntaxNode("repeat", value="comma", children=(node,), minimum=minimum, maximum=maximum)
            elif self._accept("!"):
                if node.kind != "group":
                    raise GenerationError("syntax: ! requires a group")
                node = SyntaxNode("required", children=(node,))
            elif self._accept("{"):
                minimum = self._number("repeat minimum")
                if self._accept(","):
                    maximum = None if self._peek().kind == "}" else self._number("repeat maximum")
                else:
                    maximum = minimum
                self._expect("}")
                node = SyntaxNode("repeat", children=(node,), minimum=minimum, maximum=maximum)
            else:
                return node

    def _number(self, label: str) -> int:
        token = self._peek()
        if token.kind != "word" or not token.text.isdigit():
            raise GenerationError(f"syntax at {token.position}: expected {label}")
        self.index += 1
        return int(token.text)

    def _parse_primary(self) -> SyntaxNode:
        token = self._peek()
        if token.kind == "function":
            self.index += 1
            child = self._parse_alternative()
            self._expect(")")
            return SyntaxNode("function", value=token.text, children=(child,))
        if token.kind == "[":
            self.index += 1
            child = self._parse_alternative()
            self._expect("]")
            return SyntaxNode("group", children=(child,))
        if token.kind == "(":
            self.index += 1
            child = self._parse_alternative()
            self._expect(")")
            return SyntaxNode("group", children=(child,))
        if token.kind == "type":
            self.index += 1
            return self._parse_type(token)
        if token.kind in {"word", ",", "/"}:
            self.index += 1
            return SyntaxNode("keyword" if token.kind == "word" else "literal", value=token.text)
        raise GenerationError(f"syntax at {token.position}: unexpected {token.text!r}")

    @staticmethod
    def _parse_type(token: SyntaxToken) -> SyntaxNode:
        raw = token.text
        if raw.startswith("<<") and raw.endswith(">>"):
            return SyntaxNode("type", value=raw[2:-2].strip())

        body = raw[1:-1].strip()
        match = re.fullmatch(r"([^\[]+?)(?:\[\s*([^,]+?)\s*,\s*([^\]]+?)\s*\])?", body)
        if not match:
            raise GenerationError(f"syntax at {token.position}: invalid type reference {raw!r}")
        name = match.group(1).strip()
        bounds = None if match.group(2) is None else (match.group(2).strip(), match.group(3).strip())
        return SyntaxNode("type", value=name, bounds=bounds)


def parse_syntax(source: str) -> SyntaxNode:
    return SyntaxParser(source).parse()


@dataclass(frozen=True)
class PropertyDefinition:
    name: str
    syntax: SyntaxNode | None
    syntax_parser: str | None
    type_name: str | None
    initial: str
    inherited: bool
    storage_path: tuple[str, ...]
    storage_kind: str
    storage_name: str | None
    group: tuple[str, str] | None
    values: tuple[str, ...]
    longhands: tuple[str, ...]
    reset_longhands: tuple[str, ...]
    disables_native_appearance: bool


@dataclass(frozen=True)
class Catalogs:
    properties: tuple[PropertyDefinition, ...]
    data_types: dict[str, SyntaxNode]
    css_keywords: tuple[str, ...]
    html_tags: dict[str, dict[str, Any]]
    html_attributes: tuple[str, ...]
    input_types: tuple[str, ...]
    events: dict[str, dict[str, Any]]
    pseudo_selectors: dict[str, dict[str, dict[str, Any]]]


def _validate_property(name: str, metadata: dict[str, Any], property_names: set[str], path: Path) -> PropertyDefinition:
    allowed_metadata = {"comment", "inherited", "initial", "values", "longhands", "reset_longhands", "codegen"}
    unknown_metadata = set(metadata) - allowed_metadata
    if unknown_metadata:
        raise GenerationError(f"{path}.{name}: unknown fields {sorted(unknown_metadata)}")

    codegen = _require_dict(metadata.get("codegen", {}), f"{path}.{name}.codegen")
    allowed_codegen = {
        "syntax",
        "syntax_unused",
        "syntax_unused_reason",
        "syntax_parser",
        "storage_path",
        "storage_kind",
        "storage_name",
        "group",
        "type",
        "disables_native_appearance",
    }
    unknown_codegen = set(codegen) - allowed_codegen
    if unknown_codegen:
        raise GenerationError(f"{path}.{name}.codegen: unknown fields {sorted(unknown_codegen)}")

    syntax = codegen.get("syntax")
    syntax_unused = codegen.get("syntax_unused")
    syntax_parser = codegen.get("syntax_parser")
    syntax_reason = codegen.get("syntax_unused_reason")
    longhands = tuple(_require_string_list(metadata.get("longhands", []), f"{path}.{name}.longhands"))
    reset_longhands = tuple(_require_string_list(metadata.get("reset_longhands", []), f"{path}.{name}.reset_longhands"))
    if len(set(longhands)) != len(longhands):
        raise GenerationError(f"{path}.{name}.longhands: duplicate longhand")
    if len(set(reset_longhands)) != len(reset_longhands):
        raise GenerationError(f"{path}.{name}.reset_longhands: duplicate longhand")
    if set(longhands) & set(reset_longhands):
        raise GenerationError(f"{path}.{name}: reset_longhands must not repeat a longhand")
    missing_longhands = (set(longhands) | set(reset_longhands)) - property_names
    if missing_longhands:
        raise GenerationError(f"{path}.{name}: unknown longhands {sorted(missing_longhands)}")
    if syntax is not None:
        _require_string(syntax, f"{path}.{name}.codegen.syntax")
    if syntax is not None and syntax_unused is not None:
        raise GenerationError(f"{path}.{name}: syntax and syntax_unused are mutually exclusive")
    if syntax_parser is not None:
        _require_string(syntax_parser, f"{path}.{name}.codegen.syntax_parser")
        _cpp_identifier(syntax_parser, f"{path}.{name}.codegen.syntax_parser")
        if syntax is not None:
            raise GenerationError(f"{path}.{name}: syntax_parser cannot accompany syntax")
        if syntax_unused is None:
            raise GenerationError(f"{path}.{name}: syntax_parser requires syntax_unused")
    if syntax_unused is not None:
        _require_string(syntax_unused, f"{path}.{name}.codegen.syntax_unused")
        if syntax_parser is None and not longhands:
            raise GenerationError(f"{path}.{name}: syntax_unused requires a shorthand or custom parser")
    if syntax_reason is not None:
        _require_string(syntax_reason, f"{path}.{name}.codegen.syntax_unused_reason")
        if syntax_unused is None:
            raise GenerationError(f"{path}.{name}: syntax_unused_reason requires syntax_unused")
    inherited = metadata.get("inherited", False)
    if not isinstance(inherited, bool):
        raise GenerationError(f"{path}.{name}.inherited: expected a boolean")
    comment = metadata.get("comment")
    if comment is not None:
        _require_string(comment, f"{path}.{name}.comment")
    initial = metadata.get("initial", "")
    if not isinstance(initial, str):
        raise GenerationError(f"{path}.{name}.initial: expected a string")
    catalog_only = syntax is None and syntax_unused is None and not longhands
    if catalog_only and not initial:
        raise GenerationError(f"{path}.{name}: an initial is required when the property has no implementation metadata")
    values = tuple(_require_string_list(metadata.get("values", []), f"{path}.{name}.values"))
    if syntax is not None and "<<values>>" in syntax:
        if not values:
            raise GenerationError(f"{path}.{name}.values: required when syntax is <<values>>")
        syntax = syntax.replace("<<values>>", " | ".join(values))
    parsed_syntax = parse_syntax(syntax) if syntax is not None else None
    if reset_longhands and not longhands:
        raise GenerationError(f"{path}.{name}.reset_longhands: requires longhands")
    storage_path_value = codegen.get("storage_path")
    if storage_path_value is None:
        if not longhands and syntax_parser is None and not catalog_only:
            raise GenerationError(f"{path}.{name}.codegen.storage_path: required for non-shorthand properties")
        storage_path = ()
    else:
        if not isinstance(storage_path_value, list) or not storage_path_value:
            raise GenerationError(f"{path}.{name}.codegen.storage_path: expected a non-empty member-name list")
        storage_path = tuple(_require_string_list(storage_path_value, f"{path}.{name}.codegen.storage_path"))
        for member in storage_path:
            if not PATH_MEMBER.fullmatch(member):
                raise GenerationError(f"{path}.{name}.codegen.storage_path: invalid member name {member!r}")

    storage_kind = codegen.get("storage_kind", "value")
    if not storage_path and "storage_kind" in codegen:
        raise GenerationError(f"{path}.{name}.codegen.storage_kind: shorthand properties do not have direct storage")
    if not isinstance(storage_kind, str):
        raise GenerationError(f"{path}.{name}.codegen.storage_kind: expected a string")
    if storage_kind not in STORAGE_KINDS:
        raise GenerationError(f"{path}.{name}.codegen.storage_kind: unsupported value {storage_kind!r}")
    storage_name = codegen.get("storage_name")
    type_name = codegen.get("type")
    if type_name is not None:
        _require_string(type_name, f"{path}.{name}.codegen.type")
        _cpp_identifier(type_name, f"{path}.{name}.codegen.type")
    group_data = codegen.get("group")
    group_value = None
    if group_data is not None:
        group_path = f"{path}.{name}.codegen.group"
        group = _require_dict(group_data, group_path)
        unknown_group_fields = set(group) - {"name", "member"}
        if unknown_group_fields:
            raise GenerationError(f"{group_path}: unknown fields {sorted(unknown_group_fields)}")
        group_value = (
            _require_string(group.get("name"), f"{group_path}.name"),
            _require_string(group.get("member"), f"{group_path}.member"),
        )
        if not type_name or not storage_path or longhands:
            raise GenerationError(f"{group_path}: requires a typed longhand with direct storage")
    if storage_name is not None:
        _require_string(storage_name, f"{path}.{name}.codegen.storage_name")
        _cpp_identifier(storage_name, f"{path}.{name}.codegen.storage_name")
        if not type_name or not storage_path or longhands or group_value is not None:
            raise GenerationError(f"{path}.{name}.codegen.storage_name: requires standalone typed storage")
        if storage_name == _property_member_name_from_name(name):
            raise GenerationError(f"{path}.{name}.codegen.storage_name: omit when it matches the property name")
    disables_native_appearance = codegen.get("disables_native_appearance", False)
    if disables_native_appearance is not False and disables_native_appearance is not True:
        raise GenerationError(f"{path}.{name}.codegen.disables_native_appearance: expected true when present")
    if disables_native_appearance is False and "disables_native_appearance" in codegen:
        raise GenerationError(f"{path}.{name}.codegen.disables_native_appearance: omit false")
    return PropertyDefinition(
        name=name,
        syntax=parsed_syntax,
        syntax_parser=syntax_parser,
        type_name=type_name,
        initial=initial,
        inherited=inherited,
        storage_path=storage_path,
        storage_kind=storage_kind,
        storage_name=storage_name,
        group=group_value,
        values=values,
        longhands=longhands,
        reset_longhands=reset_longhands,
        disables_native_appearance=disables_native_appearance,
    )


def _property_reference(node: SyntaxNode) -> str | None:
    if node.kind != "type" or len(node.value) < 2 or node.value[0] != "'" or node.value[-1] != "'":
        return None
    return node.value[1:-1]


def _longhand_component_syntax(longhand: PropertyDefinition, properties: dict[str, PropertyDefinition]) -> SyntaxNode:
    repeated = _repeat_spec(longhand.syntax) if longhand.syntax is not None else None
    component = repeated[0] if repeated else longhand.syntax
    reference = _property_reference(component) if component is not None else None
    if reference is not None and reference in properties and properties[reference].syntax is not None:
        return _longhand_component_syntax(properties[reference], properties)
    return component


def _map_any_order_components(
    syntax: SyntaxNode,
    property: PropertyDefinition,
    properties: dict[str, PropertyDefinition],
    path: Path,
) -> None:
    if syntax.kind == "group" and len(syntax.children) == 1:
        syntax = syntax.children[0]
    if syntax.kind != "any_order":
        raise GenerationError(f"{path}.{property.name}: expected a || grammar")

    mapped = []
    for component in syntax.children:
        reference = _property_reference(component)
        if reference is not None:
            if reference not in property.longhands:
                raise GenerationError(f"{path}.{property.name}: unknown longhand reference {reference!r}")
            mapped.append(reference)
            continue
        matches = [name for name in property.longhands if _longhand_component_syntax(properties[name], properties) == component]
        if len(matches) != 1:
            raise GenerationError(f"{path}.{property.name}: grammar component {component.render()!r} must match one longhand")
        mapped.append(matches[0])
    if len(mapped) != len(property.longhands) or set(mapped) != set(property.longhands):
        raise GenerationError(f"{path}.{property.name}: || components must map one-to-one to longhands")


def _infer_shorthand_pattern(
    property: PropertyDefinition,
    properties: dict[str, PropertyDefinition],
    path: Path,
) -> str:
    syntax = property.syntax
    if syntax is None:
        raise GenerationError(f"{path}.{property.name}: generated shorthand parser needs syntax")
    while syntax.kind == "group" and len(syntax.children) == 1:
        syntax = syntax.children[0]

    repeated = _repeat_spec(syntax)
    if repeated:
        component, minimum, maximum, separator = repeated
        if separator == ",":
            if len(property.longhands) < 2 or minimum != 1 or maximum is not None:
                raise GenerationError(f"{path}.{property.name}: comma-separated shorthand needs multiple longhands and #")
            _map_any_order_components(component, property, properties, path)
            return "Layered"
        if separator != " ":
            raise GenerationError(f"{path}.{property.name}: unsupported shorthand repetition separator")
        component_reference = _property_reference(component)
        if len(property.longhands) == 4 and (minimum, maximum) == (1, 4):
            first = property.longhands[0]
            first_syntax = _longhand_component_syntax(properties[first], properties)
            matches_first = component_reference == first or (component_reference is None and component == first_syntax)
            if matches_first and all(
                _longhand_component_syntax(properties[name], properties) == first_syntax
                for name in property.longhands
            ):
                return "CoalescingQuad"
        if len(property.longhands) == 2 and (minimum, maximum) == (1, 2):
            first = property.longhands[0]
            first_syntax = _longhand_component_syntax(properties[first], properties)
            if component_reference == first or (component_reference is None and component == first_syntax):
                return "CoalescingPair"

    if syntax.kind == "any_order":
        _map_any_order_components(syntax, property, properties, path)
        return "AnyOrder"

    if syntax.kind == "sequence":
        components = syntax.children
        if len(property.longhands) > 1 and len(components) == len(property.longhands) and all(
            _property_reference(component) == longhand for component, longhand in zip(components, property.longhands, strict=True)
        ):
            return "SpaceSeparated"

        if len(property.longhands) > 1 and len(components) == len(property.longhands) * 2 - 1 and all(
            _property_reference(components[index * 2]) == longhand
            and (index == len(property.longhands) - 1 or components[index * 2 + 1] == SyntaxNode("literal", value="/"))
            for index, longhand in enumerate(property.longhands)
        ):
            return "SlashSeparated"

        if len(components) == 2 and len(property.longhands) == 2:
            first, optional_second = components
            second_repeat = _repeat_spec(optional_second)
            if second_repeat and second_repeat[1:] == (0, 1, " "):
                if (_property_reference(first) == property.longhands[0]
                        and _property_reference(second_repeat[0]) == property.longhands[1]):
                    return "CoalescingPair"

    raise GenerationError(f"{path}.{property.name}: unsupported shorthand syntax {syntax.render()!r}; add a custom syntax_parser")


def _validate_shorthand(property: PropertyDefinition, properties: dict[str, PropertyDefinition], path: Path) -> None:
    if not property.longhands:
        if property.reset_longhands:
            raise GenerationError(f"{path}.{property.name}: reset_longhands requires longhands")
        return

    if property.syntax_parser in SHORTHAND_SYNTAX_PARSERS:
        if property.syntax is not None:
            raise GenerationError(f"{path}.{property.name}: a custom shorthand syntax_parser uses syntax_unused")
        for name in property.longhands:
            component = properties[name]
            if component.syntax is None and component.syntax_parser not in CUSTOM_LONGHAND_PARSERS:
                raise GenerationError(f"{path}.{property.name}: longhand {name!r} needs a generated parser")
    elif property.syntax is not None and property.syntax_parser is None:
        _infer_shorthand_pattern(property, properties, path)
    elif property.syntax_parser is None:
        raise GenerationError(f"{path}.{property.name}: shorthand needs syntax or syntax_parser")

    for name in property.longhands:
        component = properties[name]
        if component.longhands and component.syntax_parser is None and component.syntax is None:
            raise GenerationError(f"{path}.{property.name}: nested shorthand {name!r} needs a parser")

    for name in property.reset_longhands:
        reset = properties[name]
        has_generated_parser = (
            (reset.syntax is not None and reset.syntax_parser is None)
            or reset.syntax_parser in CUSTOM_LONGHAND_PARSERS
        )
        if (
            reset.longhands
            or not has_generated_parser
            or not reset.storage_path
            or not reset.type_name
        ):
            raise GenerationError(f"{path}.{property.name}: reset longhand {name!r} must be an implemented codegen longhand")
        if not reset.initial or len(reset.initial.split()) != 1:
            raise GenerationError(f"{path}.{property.name}: reset longhand {name!r} needs a single-value initial")

def _property_groups(properties: Iterable[PropertyDefinition]) -> dict[str, tuple[PropertyDefinition, ...]]:
    groups: dict[str, list[PropertyDefinition]] = {}
    for property in properties:
        if property.group is None:
            continue
        name, _ = property.group
        groups.setdefault(name, []).append(property)
    return {name: tuple(members) for name, members in groups.items()}


def _validate_property_groups(
    properties: dict[str, PropertyDefinition], path: Path
) -> None:
    edge_members = {"top", "right", "bottom", "left"}
    corner_members = {"top-left", "top-right", "bottom-right", "bottom-left"}
    for name, members in _property_groups(properties.values()).items():
        label = f"{path}.{name}.codegen.group"
        shorthand = properties.get(name)
        if shorthand is None or not shorthand.longhands:
            raise GenerationError(f"{label}: name must identify a shorthand property")
        group_members = [property.group[1] for property in members]
        if len(group_members) != len(set(group_members)):
            raise GenerationError(f"{label}: duplicate member")
        if set(property.name for property in members) != set(shorthand.longhands):
            raise GenerationError(f"{label}: members must match the shorthand longhands")
        if set(group_members) not in (edge_members, corner_members):
            raise GenerationError(f"{label}: members must describe all four edges or corners")
        first = members[0]
        if any(
            property.type_name != first.type_name
            or property.storage_path != first.storage_path
            or property.storage_kind != first.storage_kind
            for property in members[1:]
        ):
            raise GenerationError(f"{label}: members must share type and storage")
        if set(group_members) == corner_members and first.type_name != "CornerRadius":
            raise GenerationError(f"{label}: corner groups require CornerRadius values")


def _validate_events(events: dict[str, dict[str, Any]], path: Path) -> None:
    allowed = {"interface", "bubbles", "cancelable", "composed"}
    for name, metadata in events.items():
        unknown = set(metadata) - allowed
        if unknown:
            raise GenerationError(f"{path}.{name}: unknown fields {sorted(unknown)}")
        _require_string(metadata.get("interface"), f"{path}.{name}.interface")
        for field in ("bubbles", "cancelable", "composed"):
            if field in metadata and not isinstance(metadata[field], bool):
                raise GenerationError(f"{path}.{name}.{field}: expected a boolean")
    _validate_enum_names((_event_cpp_name(name) for name in events), f"{path} C++ event names")


def _validate_html_tags(tags: dict[str, dict[str, Any]], path: Path) -> None:
    for name, metadata in tags.items():
        unknown = set(metadata) - {"interface", "void"}
        if unknown:
            raise GenerationError(f"{path}.{name}: unknown fields {sorted(unknown)}")
        _require_string(metadata.get("interface"), f"{path}.{name}.interface")
        if "void" in metadata and metadata["void"] is not True:
            raise GenerationError(f"{path}.{name}.void: omit the field for non-void tags")
    _validate_enum_names(
        list(dict.fromkeys(["Unknown", *(metadata["interface"] for metadata in tags.values())])),
        f"{path} interface names",
    )


def _validate_pseudo_selectors(data: dict[str, dict[str, dict[str, Any]]], path: Path) -> None:
    for group_name, entries in data.items():
        if group_name not in {"pseudo-classes", "pseudo-elements"}:
            raise GenerationError(f"{path}: unknown selector group {group_name!r}")
        _validate_enum_names(entries, f"{path}.{group_name}")
        for name, metadata in entries.items():
            allowed = {"argument_requirement", "argument_syntax", "specificity", "user_agent"}
            unknown = set(metadata) - allowed
            if unknown:
                raise GenerationError(f"{path}.{group_name}.{name}: unknown fields {sorted(unknown)}")
            if "argument_requirement" in metadata:
                _require_string(metadata["argument_requirement"], f"{path}.{group_name}.{name}.argument_requirement")
                if metadata["argument_requirement"] not in {"optional", "required"}:
                    raise GenerationError(
                        f"{path}.{group_name}.{name}.argument_requirement: expected 'optional' or 'required'"
                    )
            if "argument_syntax" in metadata:
                _require_string(metadata["argument_syntax"], f"{path}.{group_name}.{name}.argument_syntax")
            if ("argument_requirement" in metadata) != ("argument_syntax" in metadata):
                raise GenerationError(
                    f"{path}.{group_name}.{name}: argument_requirement and argument_syntax must be declared together"
                )
            if "argument_syntax" in metadata and metadata["argument_syntax"] not in {
                "ident",
                "compound-selector",
                "forgiving-selector-list",
            }:
                raise GenerationError(f"{path}.{group_name}.{name}.argument_syntax: unsupported pseudo-class argument syntax")
            if metadata.get("specificity", "class") not in {"class", "argument", "class-plus-argument", "zero"}:
                raise GenerationError(
                    f"{path}.{group_name}.{name}.specificity: expected 'class', 'argument', "
                    "'class-plus-argument', or 'zero'"
                )
            if "user_agent" in metadata and not isinstance(metadata["user_agent"], bool):
                raise GenerationError(f"{path}.{group_name}.{name}.user_agent: expected a boolean")


def load_catalogs(source_root: Path) -> Catalogs:
    core_dir = source_root / "Core"
    css_dir = core_dir / "css"
    css_values_dir = css_dir / "values"
    html_dir = core_dir / "html"
    event_dir = core_dir / "dom"
    properties_path = css_dir / "CSSProperties.json5"
    properties_catalog = _require_dict(load_json5(properties_path), str(properties_path))
    data_types = _data_type_map(properties_catalog.get("data_types", {}), f"{properties_path}.data_types")
    properties_data = _require_dict(properties_catalog.get("properties"), f"{properties_path}.properties")
    property_names = set(properties_data)
    properties = tuple(
        _validate_property(name, _require_dict(metadata, f"{properties_path}.{name}"), property_names, properties_path)
        for name, metadata in properties_data.items()
    )
    properties_by_name = {property.name: property for property in properties}
    _validate_property_groups(properties_by_name, properties_path)
    for property in properties:
        _validate_shorthand(property, properties_by_name, properties_path)

    css_keywords_path = css_values_dir / "CSSKeywords.json5"
    html_tags_path = html_dir / "HTMLTags.json5"
    html_attributes_path = html_dir / "HTMLAttributes.json5"
    input_types_path = html_dir / "InputTypes.json5"
    events_path = event_dir / "EventTypes.json5"
    css_keywords_data = _name_map(load_json5(css_keywords_path), css_keywords_path)
    html_tags = _name_map(load_json5(html_tags_path), html_tags_path)
    html_attributes = tuple(_name_map(load_json5(html_attributes_path), html_attributes_path))
    input_types = tuple(_name_map(load_json5(input_types_path), input_types_path))
    events_data = _require_dict(load_json5(events_path), str(events_path))
    pseudo_path = css_dir / "CSSPseudoSelectors.json5"
    pseudo_data = _require_dict(load_json5(pseudo_path), str(pseudo_path))

    _validate_enum_names(properties_data, f"{properties_path}.properties")
    _validate_enum_names(css_keywords_data, str(css_keywords_path))
    _validate_enum_names(html_tags, str(html_tags_path))
    _validate_enum_names((_html_attribute_cpp_name(name) for name in html_attributes), str(html_attributes_path))
    _validate_enum_names(input_types, str(input_types_path))
    _validate_html_tags(html_tags, html_tags_path)
    _validate_events(events_data, events_path)
    _validate_pseudo_selectors(pseudo_data, pseudo_path)
    return Catalogs(
        properties=properties,
        data_types=data_types,
        css_keywords=tuple(css_keywords_data),
        html_tags=html_tags,
        html_attributes=html_attributes,
        input_types=input_types,
        events=events_data,
        pseudo_selectors=pseudo_data,
    )


def _banner(source: str) -> str:
    return "// Automatically generated from " + source + ", do not edit.\n\n"


def _single_statement_function(signature: str, statement: str) -> str:
    line = f"{signature} {{ {statement} }}"
    if "\n" not in statement and len(line) <= 140:
        return line
    return f"{signature} {{\n    {statement}\n}}"


def _enum_header(
    enum_name: str,
    items: Iterable[str],
    *,
    underlying: str = "std::uint16_t",
    prefix: Iterable[str] = (),
    include_count: bool = True,
) -> str:
    names = list(items)
    lines = [f"enum class {enum_name} : {underlying} {{"]
    lines.extend(f"    {name}," for name in prefix)
    lines.extend(f"    {_pascal_name(name)}," for name in names)
    if include_count:
        lines.append("    Count")
    lines.append("};")
    return "\n".join(lines)


def generate_property_names(catalogs: Catalogs) -> tuple[str, str]:
    names = [property.name for property in catalogs.properties]
    header = _banner("CSSProperties.json5") + (
        "#pragma once\n\n"
        "#include <cstddef>\n"
        "#include <cstdint>\n"
        "#include <iterator>\n"
        "#include <optional>\n"
        "#include <string_view>\n\n"
        "namespace Core::CSS {\n"
    )
    header += _enum_header("Property", names)
    header += "\n\nstruct PropertyDescriptor {\n    Property property;\n    std::string_view name;\n};\n"
    header += "\ninline constexpr PropertyDescriptor properties[] {\n"
    for name in names:
        header += f"    {{Property::{_pascal_name(name)}, {_cpp_string(name)}}},\n"
    header += "};\n\n"
    header += "constexpr const PropertyDescriptor* propertyDescriptor(Property property) {\n"
    header += "    const auto index = static_cast<std::size_t>(property);\n"
    header += "    return index < std::size(properties) ? &properties[index] : nullptr;\n"
    header += "}\n\n"
    header += "constexpr std::string_view propertyName(Property property) {\n"
    header += (
        "    if (const auto* descriptor = propertyDescriptor(property))\n"
        "        return descriptor->name;\n"
    )
    header += "    return {};\n"
    header += "}\n\nstd::optional<Property> findProperty(std::string_view);\n} // namespace Core::CSS\n"

    cpp = _banner("CSSProperties.json5") + '#include "CSSProperties.h"\n\nnamespace Core::CSS {\n'
    cpp += "std::optional<Property> findProperty(std::string_view name) {\n"
    cpp += "    for (const auto& descriptor : properties)\n"
    cpp += (
        "        if (descriptor.name == name)\n"
        "            return descriptor.property;\n"
    )
    cpp += "    return std::nullopt;\n"
    cpp += "}\n} // namespace Core::CSS\n"
    return header, cpp


def generate_keyword_names(catalogs: Catalogs) -> tuple[str, str]:
    names = list(catalogs.css_keywords)
    enum_names = {
        name: f"Keyword{_pascal_name(name, reserved=False)}"
        for name in names
    }
    system_color_names = SYSTEM_COLORS
    missing_system_colors = set(system_color_names) - set(names)
    if missing_system_colors:
        raise GenerationError(
            "system color keywords are missing from CSSKeywords.json5: "
            f"{sorted(missing_system_colors)}"
        )
    header = _banner("CSSKeywords.json5") + (
        "#pragma once\n\n"
        "#include <cstddef>\n"
        "#include <cstdint>\n"
        "#include <iterator>\n"
        "#include <optional>\n"
        "#include <string_view>\n\n"
        "namespace Core::CSS {\n"
    )
    header += "enum KeywordName : std::uint16_t {\n"
    header += "".join(f"    {name},\n" for name in enum_names.values())
    header += "};"
    header += (
        "\n\ntemplate<KeywordName C> struct Constant {\n"
        "    static constexpr auto value = C;\n"
        "    constexpr operator KeywordName() const { return C; }\n"
        "    constexpr bool operator==(const Constant&) const = default;\n"
        "};\n\n"
        "struct Keyword {\n"
        "    KeywordName keyword;\n\n"
        "    constexpr bool operator==(const Keyword&) const = default;\n"
        "    constexpr bool operator==(KeywordName other) const { return keyword == other; }\n\n"
    )
    header += "\n".join(
        f"    using {_pascal_name(name)} = Constant<{enum_names[name]}>;"
        for name in names
    )
    header += "\n};"
    header += "\n\nstruct KeywordDescriptor {\n    KeywordName keyword;\n    std::string_view name;\n};\n"
    header += "\ninline constexpr KeywordDescriptor keywords[] {\n"
    for name in names:
        header += f"    {{{enum_names[name]}, {_cpp_string(name)}}},\n"
    header += "};\n\n"
    header += "constexpr const KeywordDescriptor* keywordDescriptor(KeywordName keyword) {\n"
    header += "    const auto index = static_cast<std::size_t>(keyword);\n"
    header += "    return index < std::size(keywords) ? &keywords[index] : nullptr;\n"
    header += "}\n\n"
    header += "constexpr std::string_view keywordName(KeywordName keyword) {\n"
    header += (
        "    if (const auto* descriptor = keywordDescriptor(keyword))\n"
        "        return descriptor->name;\n"
    )
    header += "    return {};\n"
    header += "}\n\n"
    header += "constexpr bool isSystemColorKeyword(KeywordName keyword) {\n    switch (keyword) {\n"
    for name in system_color_names:
        header += f"    case {enum_names[name]}:\n"
    header += "        return true;\n"
    header += "    default:\n        return false;\n    }\n}\n\n"
    header += "std::optional<KeywordName> findKeyword(std::string_view);\n} // namespace Core::CSS\n"
    cpp = _banner("CSSKeywords.json5") + '#include "CSSKeywords.h"\n\nnamespace Core::CSS {\n'
    cpp += "std::optional<KeywordName> findKeyword(std::string_view name) {\n"
    cpp += "    for (const auto& descriptor : keywords)\n"
    cpp += (
        "        if (descriptor.name == name)\n"
        "            return descriptor.keyword;\n"
    )
    cpp += "    return std::nullopt;\n"
    cpp += "}\n} // namespace Core::CSS\n"
    return header, cpp


def _property_member_name(property: PropertyDefinition) -> str:
    return _property_member_name_from_name(property.name)


def _property_member_name_from_name(name: str) -> str:
    member_name = _pascal_name(name, reserved=False)
    return member_name[0].lower() + member_name[1:]


def _property_storage_path(property: PropertyDefinition) -> tuple[str, ...]:
    if property.group is not None:
        return (*property.storage_path, _computed_style_member(property.group[1]))
    member_name = property.storage_name or _property_member_name(property)
    return property.storage_path if property.storage_path[-1] == member_name else (*property.storage_path, member_name)


def _font_cascade_accessor(property: PropertyDefinition) -> str | None:
    if property.storage_path != ("mInheritedData", "fontData", "fontCascade"):
        return None
    if property.name not in {"font-family", "font-size", "font-style", "font-weight", "font-width"}:
        return None
    return property.storage_name or _property_member_name(property)


def _storage_expression(path: tuple[str, ...], *, mutation: bool = False) -> str:
    expression = path[0]
    if mutation:
        if path[0] in REF_STORAGE_MEMBERS:
            expression += ".access()"
        for member in path[1:]:
            expression += "." + member
            if member in REF_STORAGE_MEMBERS:
                expression += ".access()"
    else:
        for previous, member in zip(path, path[1:]):
            expression += ("->" if previous in REF_STORAGE_MEMBERS else ".") + member
    return expression


def _computed_style_member(member: str) -> str:
    name = _pascal_name(member, reserved=False)
    return name[0].lower() + name[1:]


def _accessor_return_type(property: PropertyDefinition) -> str:
    if property.storage_kind == "reference":
        return f"const {property.type_name}&"
    return property.type_name


def _property_group_lines(catalogs: Catalogs) -> list[str]:
    edge_members = ("top", "right", "bottom", "left")
    corner_members = ("top-left", "top-right", "bottom-right", "bottom-left")
    lines: list[str] = []
    for group_name, properties in sorted(_property_groups(catalogs.properties).items()):
        members = {property.group[1]: property for property in properties}
        if set(members) == set(edge_members):
            member_order = edge_members
            group_type = f"Layout::RectEdges<{properties[0].type_name}>"
        elif set(members) == set(corner_members):
            member_order = corner_members
            group_type = "BorderRadius"
        else:
            raise GenerationError(f"{group_name}: unsupported computed style group members")

        setter_name = _pascal_name(group_name, reserved=False)
        getter_name = setter_name[0].lower() + setter_name[1:]
        getter_values = ", ".join(f"{_property_member_name(members[member])}()" for member in member_order)
        lines.extend(
            [
                _single_statement_function(f"{group_type} {getter_name}() const", f"return {{{getter_values}}};"),
                "",
                f"void set{setter_name}({group_type} value) {{",
                *(
                    f"    set{_pascal_name(members[member].name)}(std::move(value.{_computed_style_member(member)}));"
                    for member in member_order
                ),
                "}",
            ]
        )
        if member_order == edge_members:
            value_type = properties[0].type_name
            uniform_values = ", ".join("value" for _ in member_order)
            lines.extend(
                [
                    "",
                    _single_statement_function(
                        f"void set{setter_name}({value_type} value)",
                        f"set{setter_name}({group_type} {{{uniform_values}}});",
                    ),
                ]
            )
        lines.append("")
    return lines


def _storage_read_expression(property: PropertyDefinition, expression: str) -> str:
    if accessor := _font_cascade_accessor(property):
        return f"{expression}.{accessor}()"
    if property.storage_kind == "enum":
        return f"static_cast<{property.type_name}>({expression})"
    if property.storage_kind == "raw":
        return f"{property.type_name}::fromRaw({expression})"
    return expression


def _storage_write_statement(property: PropertyDefinition, expression: str) -> str:
    if accessor := _font_cascade_accessor(property):
        setter = "set" + _pascal_name(accessor, reserved=False)
        return f"{expression}.{setter}(std::move(value));"
    if property.storage_kind == "enum":
        return f"{expression} = static_cast<unsigned>(value);"
    if property.storage_kind == "raw":
        return f"{expression} = value.toRaw();"
    if property.storage_kind == "reference":
        return f"{expression} = std::move(value);"
    return f"{expression} = value;"


def _storage_setter_lines(property: PropertyDefinition, expression: str) -> list[str]:
    if accessor := _font_cascade_accessor(property):
        comparison = f"value != {expression}.{accessor}()"
    elif property.storage_kind == "enum":
        comparison = f"value != static_cast<{property.type_name}>({expression})"
    elif property.storage_kind == "raw":
        comparison = f"value != {property.type_name}::fromRaw({expression})"
    else:
        comparison = f"value != {expression}"
    path = property.storage_path if _font_cascade_accessor(property) else _property_storage_path(property)
    mutation_expression = _storage_expression(path, mutation=True)
    return [
        f"    if ({comparison})",
        f"        {_storage_write_statement(property, mutation_expression)}",
    ]


def _initial_member_specifier(property: PropertyDefinition) -> str:
    if _initial_keyword(property) or property.type_name in {
        "BorderStyle", "Dimension", "FlexWrap", "LetterSpacing", "MaximumSize", "MinimumSize", "OptionalDimension",
        "PaddingEdge", "PreferredSize", "WordSpacing"
    }:
        return "static constexpr"
    initial = INITIAL_VALUE.fullmatch(property.initial)
    if property.storage_kind == "enum" or (initial and initial.group(2) is None):
        return "static constexpr"
    return "static"


def _initial_keyword(property: PropertyDefinition) -> str | None:
    if property.name == "font-size" and property.initial == "medium":
        return property.initial
    if property.type_name == "Color" and property.initial in {"currentcolor", "transparent"}:
        return property.initial
    if property.type_name in {"FontWeight", "FontWidth"} and property.initial == "normal":
        return property.initial
    if property.type_name == "LineWidth" and property.initial in {"thin", "medium", "thick"}:
        return property.initial
    if property.type_name == "LineHeight" and property.initial == "normal":
        return property.initial
    return None


def _initial_return_type(property: PropertyDefinition) -> str:
    return "auto" if _initial_keyword(property) else property.type_name


def _inline_property_operations(catalogs: Catalogs) -> str:
    lines: list[str] = []
    for property in _typed_properties(catalogs):
        name = _pascal_name(property.name, reserved=False)
        member_name = _property_member_name(property)
        path = property.storage_path if _font_cascade_accessor(property) else _property_storage_path(property)
        expression = _storage_expression(path)
        read_expression = _storage_read_expression(property, expression)
        lines.extend(
            [
                _single_statement_function(
                    f"{_accessor_return_type(property)} {member_name}() const",
                    f"return {read_expression};",
                ),
                "",
                f"void set{name}({property.type_name} value) {{",
                *_storage_setter_lines(property, expression),
                "}",
                "",
            ]
        )
        lines.extend(
            [
                _single_statement_function(
                    f"{_initial_member_specifier(property)} {_initial_return_type(property)} initial{name}()",
                    _initial_expression(property, catalogs.css_keywords).strip(),
                ),
                "",
            ]
        )
    lines.extend(_property_group_lines(catalogs))
    return "\n".join(lines).rstrip("\n")


def generate_computed_style(catalogs: Catalogs) -> tuple[str, str, str]:
    header = _banner("CSSProperties.json5") + """#pragma once

#include <Core/CSSRules.h>
#include <cstdint>
#include <optional>
#include <span>
#include "CSSProperties.h"
#include "CSSPropertyParsing.h"

namespace Core::Style {
struct ComputedStyle;
struct BuilderState;
struct ShorthandDescriptor {
    CSS::Property property;
    std::span<const CSS::Property> properties;
};

template<CSS::Property property> constexpr ShorthandDescriptor shorthand() {
    using Traits = CSS::PropertyTraits<property>;
    return {property, Traits::Longhands::kProperties};
}

enum class ApplyType : std::uint8_t {
    Value,
    Initial,
    Inherit
};

bool isInheritedProperty(CSS::Property);
bool applyProperty(CSS::Property, BuilderState&, ApplyType, const CSS::StyleValue* specifiedValue = nullptr);
    """
    header = header.rstrip() + "\n"
    shorthands = [property for property in catalogs.properties if property.longhands]
    if shorthands:
        header += "std::optional<ShorthandDescriptor> shorthand(CSS::Property);\n"
    header += "} // namespace Core::Style\n"

    inline_header = (
        _banner("CSSProperties.json5")
        + "using enum CSS::KeywordName;\n\n"
        + _inline_property_operations(catalogs)
        + "\n"
    )

    cpp = _banner("CSSProperties.json5") + """#include "ComputedStyleProperties.h"
#include <utility>
#include <variant>
#include "ComputedStyle.h"
#include "StyleProperty.h"

namespace Core::Style {
using enum CSS::KeywordName;

"""
    keyword_converters = _keyword_enum_converters(catalogs)
    cpp += "bool isInheritedProperty(CSS::Property property) {\n    switch (property) {\n"
    inherited_properties = [property for property in catalogs.properties if property.inherited]
    for property in inherited_properties:
        cpp += f"    case CSS::Property::{_pascal_name(property.name)}:\n"
    if inherited_properties:
        cpp += "        return true;\n"
    cpp += "    default:\n        return false;\n    }\n}\n\n"
    cpp += _property_operations(catalogs, keyword_converters) + "\n} // namespace Core::Style\n"
    return header, cpp, inline_header


def _primitive_parser(node: SyntaxNode, owner: str) -> str | None:
    if node.kind != "type":
        return None
    name = node.value.lower()
    if name == "color":
        if node.bounds is not None:
            raise GenerationError(f"{owner}: range bounds are unsupported for <color>")
        return "consumeColor"
    if node.bounds is None:
        constraint = "kAnyRange"
    elif node.bounds == ("0", "inf"):
        constraint = "kNonnegative"
    else:
        minimum, maximum = node.bounds

        def range_value(value: str) -> Decimal:
            if value == "-inf":
                return Decimal("-Infinity")
            if value == "inf":
                return Decimal("Infinity")
            if not RANGE_BOUND.fullmatch(value):
                raise GenerationError(f"{owner}: invalid numeric range bound {value!r}")
            try:
                number = Decimal(value)
                float_value = struct.unpack("f", struct.pack("f", float(number)))[0]
            except (DecimalException, OverflowError) as error:
                raise GenerationError(f"{owner}: numeric range bound is outside the generated float range: {value!r}") from error
            if not math.isfinite(float_value) or (number != 0 and float_value == 0.0):
                raise GenerationError(f"{owner}: numeric range bound is outside the generated float range: {value!r}")
            return number

        if range_value(minimum) > range_value(maximum):
            raise GenerationError(f"{owner}: range lower bound {minimum} exceeds upper bound {maximum}")

        def float_literal(value: str) -> str:
            if value == "-inf":
                return "-Range::infinity"
            if value == "inf":
                return "Range::infinity"
            if "." not in value and "e" not in value.lower():
                value += ".0"
            return f"{value}f"
        constraint = f"{{{float_literal(minimum)}, {float_literal(maximum)}}}"
    mapping = {
        "integer": f"consumeInteger<{constraint}>",
        "number": f"consumeNumber<{constraint}>",
        "length": f"consumeLength<{constraint}>",
        "percentage": f"consumePercentage<{constraint}>",
        "length-percentage": f"consumeLengthPercentage<{constraint}>",
    }
    return mapping.get(name)


def _repeat_spec(node: SyntaxNode) -> tuple[SyntaxNode, int, int | None, str] | None:
    if node.kind != "repeat" or node.minimum is None:
        return None
    if node.minimum < 0 or (node.maximum is not None and node.minimum > node.maximum):
        return None
    separator = "," if node.value == "comma" else " "
    return node.children[0], node.minimum, node.maximum, separator


def _repeat_parser_expression(node: SyntaxNode, catalogs: Catalogs, owner: str) -> str | None:
    parser = _parser_expression(owner, node.children[0], catalogs)
    if parser is None or node.minimum is None:
        return None
    if parser == "consumeColor":
        parser = "detail::parseColor"

    minimum, maximum = node.minimum, node.maximum
    if node.value == "comma":
        if (minimum, maximum) == (1, None):
            return f"parseHash<{parser}>"
        if minimum == maximum:
            return f"parseHash<{minimum}, {parser}>"
        upper = "Infinite" if maximum is None else str(maximum)
        return f"parseHash<{{{minimum}, {upper}}}, {parser}>"
    if (minimum, maximum) == (0, 1):
        return f"parseOptional<{parser}>"
    if (minimum, maximum) == (0, None):
        return f"parseStar<{parser}>"
    if (minimum, maximum) == (1, None):
        return f"parsePlus<{parser}>"
    if minimum == maximum:
        return f"parseExactly<{minimum}, {parser}>"
    upper = "Infinite" if maximum is None else str(maximum)
    return f"parseRange<{{{minimum}, {upper}}}, {parser}>"


def _keyword_parser_expression(keywords: Iterable[str]) -> str:
    names = tuple(keywords)
    if len(names) == 1:
        return f"consumeKeyword<Keyword::{_pascal_name(names[0])}>"
    arguments = [f"Keyword::{_pascal_name(name)}" for name in names]
    expression = "consumeKeyword<" + ", ".join(arguments) + ">"
    if len(expression) <= 120:
        return expression
    return "consumeKeyword<\n        " + ",\n        ".join(arguments) + ">"


def _data_type_key(node: SyntaxNode) -> str | None:
    if node.kind != "type":
        return None
    return f"<{node.value}>"


def _format_parser_template_arguments(parsers: list[str]) -> str:
    return ",\n        ".join(parser.replace("\n", "\n    ") for parser in parsers)


def _parser_arguments(
    owner: str,
    nodes: tuple[SyntaxNode, ...],
    catalogs: Catalogs,
    *,
    combine_keywords: bool,
) -> list[str] | None:
    parsers = []
    index = 0
    while index < len(nodes):
        node = nodes[index]
        if node.kind == "keyword":
            keywords = []
            while index < len(nodes) and nodes[index].kind == "keyword":
                keyword = nodes[index].value
                if keyword not in catalogs.css_keywords:
                    raise GenerationError(f"{owner}: keyword {keyword!r} is missing from CSSKeywords.json5")
                keywords.append(keyword)
                index += 1
                if not combine_keywords:
                    break
            parsers.append(_keyword_parser_expression(keywords))
            continue

        parser = _parser_expression(owner, node, catalogs)
        if parser is None:
            return None
        parsers.append(parser)
        index += 1
    return ["detail::parseColor" if parser == "consumeColor" else parser for parser in parsers]


def _parser_expression(property: PropertyDefinition | str, node: SyntaxNode, catalogs: Catalogs) -> str | None:
    owner = property.name if isinstance(property, PropertyDefinition) else property
    if node.kind == "type" and node.bounds is not None:
        parser = _primitive_parser(node, owner)
        if parser is None:
            raise GenerationError(f"{owner}: range bounds are unsupported for <{node.value}>")
        return parser

    reference = _property_reference(node)
    if reference is not None:
        referenced = next((item for item in catalogs.properties if item.name == reference), None)
        if referenced is None:
            raise GenerationError(f"{owner}: unknown property reference {reference!r}")
        if referenced.longhands:
            raise GenerationError(f"{owner}: property reference {reference!r} needs a generated longhand parser")
        if referenced.syntax_parser in CUSTOM_LONGHAND_PARSERS:
            return referenced.syntax_parser
        if referenced.syntax is None or referenced.syntax_parser is not None:
            raise GenerationError(f"{owner}: property reference {reference!r} needs a generated longhand parser")
        return f"parse{_pascal_name(reference)}"

    data_type = _data_type_key(node)
    if data_type in catalogs.data_types:
        return _type_parser_name(node, catalogs)
    if node.kind == "type" and node.value.endswith("()"):
        return f"consumeFunction<{_cpp_string(node.value[:-2])}>"
    if node.kind == "keyword":
        if node.value not in catalogs.css_keywords:
            raise GenerationError(f"{owner}: keyword {node.value!r} is missing from CSSKeywords.json5")
        return _keyword_parser_expression((node.value,))
    if node.kind == "literal":
        if len(node.value) != 1:
            return None
        return f"consumeLiteral<'{node.value}'>"
    if node.kind == "repeat":
        return _repeat_parser_expression(node, catalogs, owner)
    if node.kind == "group" and len(node.children) == 1:
        return _parser_expression(owner, node.children[0], catalogs)
    if node.kind == "required" and len(node.children) == 1:
        parser = _parser_expression(owner, node.children[0], catalogs)
        return None if parser is None else f"parseRequired<{parser}>"
    if node.kind == "sequence":
        parsers = [_parser_expression(owner, child, catalogs) for child in node.children]
        if any(parser is None for parser in parsers):
            return None
        arguments = [parser for parser in parsers if parser is not None]
        return "parseSequence<\n        " + _format_parser_template_arguments(arguments) + ">"
    if node.kind == "function" and len(node.children) == 1:
        parser = _parser_expression(owner, node.children[0], catalogs)
        return None if parser is None else f"parseFunction<{_cpp_string(node.value)}, {parser}>"
    combinators = {
        "alternative": "parseOneOf",
        "any_order": "parseOneOrMoreAnyOrder",
        "all_order": "parseAllAnyOrder",
    }
    if node.kind in combinators:
        parsers = _parser_arguments(owner, node.children, catalogs, combine_keywords=node.kind == "alternative")
        if parsers is None:
            return None
        if len(parsers) == 1:
            return parsers[0]
        return f"{combinators[node.kind]}<\n        " + _format_parser_template_arguments(parsers) + ">"
    return _primitive_parser(node, owner)


def _parser_call(property: PropertyDefinition, node: SyntaxNode, catalogs: Catalogs, range_name: str) -> str:
    parser = _parser_expression(property, node, catalogs)
    if parser is None:
        raise GenerationError(
            f"{property.name}: unsupported syntax component {node.render()!r}; "
            "use syntax_unused with syntax_parser for handwritten parsing"
        )
    return f"{parser}({range_name})"


def _data_type_parser_lines(catalogs: Catalogs, data_type: str) -> list[str]:
    branches = _alternative_children(catalogs.data_types[data_type])
    parsers = _parser_arguments(data_type, branches, catalogs, combine_keywords=True)
    if not parsers:
        raise GenerationError(f"{data_type}: generated data type consumer has no alternatives")
    if len(parsers) == 1:
        parser = parsers[0]
        if parser == "detail::parseColor":
            parser = "consumeColor"
        return [f"return {parser}(range);"]
    return ["return parseOneOf<\n        " + _format_parser_template_arguments(parsers) + ">(range);"]


def _type_parser_name(node: SyntaxNode, catalogs: Catalogs) -> str | None:
    data_type = _data_type_key(node)
    if data_type not in catalogs.data_types:
        return None
    return f"consume{_pascal_name(node.value)}"


def _alternative_children(node: SyntaxNode) -> tuple[SyntaxNode, ...]:
    if node.kind == "alternative":
        return node.children
    return (node,)


def _data_type_keyword_names(catalogs: Catalogs, data_type: str) -> tuple[str, ...]:
    branches = _alternative_children(catalogs.data_types[data_type])
    if not branches or not all(branch.kind == "keyword" for branch in branches):
        raise GenerationError(
            f"{data_type}: generated consume parsers currently require keyword alternatives"
        )
    keywords = tuple(branch.value for branch in branches)
    missing = set(keywords) - set(catalogs.css_keywords)
    if missing:
        raise GenerationError(f"{data_type}: keywords missing from CSSKeywords.json5: {sorted(missing)}")
    return keywords


def _enum_member_name(keyword: str) -> str:
    if keyword == "nowrap":
        return "NoWrap"
    return _pascal_name(keyword)


def _keyword_enum_converters(catalogs: Catalogs) -> dict[str, tuple[str, ...]]:
    converters: dict[str, tuple[str, ...]] = {}
    for property in catalogs.properties:
        if property.type_name is None or property.syntax is None:
            continue
        if property.storage_kind != "enum" and property.type_name not in {"BorderStyle", "FontFamily", "FontStyle"}:
            continue
        repeat = _repeat_spec(property.syntax)
        component = repeat[0] if repeat else property.syntax
        data_type = _data_type_key(component)
        if data_type in catalogs.data_types:
            keywords = _data_type_keyword_names(catalogs, data_type)
        else:
            branches = _alternative_children(property.syntax)
            if not branches or not all(branch.kind == "keyword" for branch in branches):
                continue
            keywords = tuple(branch.value for branch in branches)
        previous = converters.get(property.type_name)
        if previous is not None and previous != keywords:
            raise GenerationError(
                f"{property.type_name}: generated keyword mappings differ between properties "
                f"({previous!r} and {keywords!r})"
            )
        converters[property.type_name] = keywords

    return converters


def _keyword_enum_converter_lines(type_name: str, keywords: tuple[str, ...]) -> list[str]:
    return [
        f"template<> constexpr std::optional<{type_name}> "
        f"fromCSSKeyword<{type_name}>(CSS::KeywordName keyword) {{",
        "    switch (keyword) {",
        *[
            f"    case Keyword{_pascal_name(keyword, reserved=False)}:\n"
            f"        return {type_name}::{_enum_member_name(keyword)};"
            for keyword in keywords
        ],
        "    default:\n        return std::nullopt;",
        "    }",
        "}",
    ]


def _initial_css_value_expression(
    property: PropertyDefinition,
    css_keywords: Iterable[str],
) -> str:
    initial = property.initial
    if not initial or len(initial.split()) != 1:
        raise GenerationError(f"{property.name}: shorthand component needs a single-value initial")
    if initial in css_keywords:
        return f"Keyword{_pascal_name(initial, reserved=False)}"
    return f"Value {{{_initial_atom(initial, css_keywords, property.name, css_namespace='')}}}"


def _syntax_nodes(node: SyntaxNode) -> Iterable[SyntaxNode]:
    yield node
    for child in node.children:
        yield from _syntax_nodes(child)


def generate_property_parsing(catalogs: Catalogs) -> tuple[str, str]:
    properties_by_name = {property.name: property for property in catalogs.properties}
    longhand_parsers = sorted(
        (
            property
            for property in catalogs.properties
            if not property.longhands
            and (
                (property.syntax is not None and property.syntax_parser is None)
                or property.syntax_parser in CUSTOM_LONGHAND_PARSERS
            )
        ),
        key=lambda property: property.name,
    )
    generated_shorthands = sorted(
        (
            property
            for property in catalogs.properties
            if property.longhands
            and (property.syntax is not None and property.syntax_parser is None or property.syntax_parser in SHORTHAND_SYNTAX_PARSERS)
        ),
        key=lambda property: property.name,
    )
    property_parsers = sorted((*longhand_parsers, *generated_shorthands), key=lambda property: property.name)
    data_type_parsers: dict[str, str] = {}
    for property in property_parsers:
        if property.syntax is None:
            continue
        for node in _syntax_nodes(property.syntax):
            data_type = _data_type_key(node)
            if data_type not in catalogs.data_types:
                continue
            parser_name = _type_parser_name(node, catalogs)
            if parser_name is None:
                continue
            previous = data_type_parsers.get(parser_name)
            if previous is not None and previous != data_type:
                raise GenerationError(f"data types {previous!r} and {data_type!r} both generate {parser_name}")
            data_type_parsers[parser_name] = data_type
    for property in property_parsers:
        if property.syntax is None:
            continue
        for node in _syntax_nodes(property.syntax):
            reference = _property_reference(node)
            if reference is None:
                continue
            referenced = properties_by_name.get(reference)
            if referenced is None or referenced.longhands or (
                referenced.syntax is None and referenced.syntax_parser not in CUSTOM_LONGHAND_PARSERS
            ):
                raise GenerationError(f"{property.name}: property reference {reference!r} needs a generated longhand parser")

    initial_properties = {
        name
        for shorthand in generated_shorthands
        for name in (*shorthand.longhands, *shorthand.reset_longhands)
    }
    for shorthand in generated_shorthands:
        for name in shorthand.longhands:
            component = properties_by_name.get(name)
            if component is None:
                raise GenerationError(f"{shorthand.name}: unknown longhand {name!r}")
            if component.longhands:
                if component not in generated_shorthands:
                    raise GenerationError(f"{shorthand.name}: shorthand component {name!r} has no generated parser")
            elif component.name not in {item.name for item in longhand_parsers}:
                raise GenerationError(f"{shorthand.name}: longhand {name!r} has no generated parser")

    header = _banner("CSSProperties.json5") + """#pragma once

#include <Core/CSSPropertyParser.h>

namespace Core::CSS {
"""
    for parser_name in sorted(data_type_parsers):
        header += f"std::optional<Value> {parser_name}(ValueRange&);\n"
    for property in longhand_parsers:
        if property.syntax_parser is None:
            header += f"std::optional<Value> parse{_pascal_name(property.name)}(ValueRange&);\n"

    for property in longhand_parsers:
        name = _pascal_name(property.name)
        header += f"template<> struct PropertyTraits<Property::{name}> {{\n"
        parser_name = property.syntax_parser or f"parse{name}"
        header += f"    static constexpr LonghandParser parser = {parser_name};\n"
        if property.name in initial_properties:
            initial = _initial_css_value_expression(property, catalogs.css_keywords)
            header += f"    static constexpr auto initial = {initial};\n"
        header += "};\n\n"

    emitted_shorthands: set[str] = set()

    def emit_shorthand_trait(property: PropertyDefinition) -> None:
        nonlocal header
        if property.name in emitted_shorthands:
            return
        for name in property.longhands:
            component = properties_by_name[name]
            if component.longhands:
                emit_shorthand_trait(component)

        name = _pascal_name(property.name)
        longhands = ",\n        ".join(
            f"Property::{_pascal_name(longhand)}" for longhand in property.longhands
        )
        header += f"template<> struct PropertyTraits<Property::{name}> {{\n"
        header += f"    using Longhands = PropertyList<\n        {longhands}>;\n"
        if property.reset_longhands:
            reset_longhands = ",\n        ".join(
                f"Property::{_pascal_name(longhand)}"
                for longhand in property.reset_longhands
            )
            header += f"    using ResetLonghands = PropertyList<\n        {reset_longhands}>;\n"
        if property.syntax_parser in SHORTHAND_SYNTAX_PARSERS:
            header += f"    using SyntaxParser = {_pascal_name(property.syntax_parser)};\n"
        elif property.syntax is not None:
            pattern = _infer_shorthand_pattern(property, properties_by_name, Path("CSSProperties.json5"))
            header += f"    using ShorthandPattern = {pattern};\n"
        header += "};\n\n"
        emitted_shorthands.add(property.name)

    for property in generated_shorthands:
        emit_shorthand_trait(property)
    for property in sorted((item for item in catalogs.properties if item.longhands), key=lambda item: item.name):
        emit_shorthand_trait(property)

    header += "bool parseProperty(Property, ValueRange&, ParsedProperties&);\n"
    header += "} // namespace Core::CSS\n"

    cpp = _banner("CSSProperties.json5") + """#include "CSSPropertyParsing.h"
#include "CSSKeywords.h"

namespace Core::CSS {
"""

    for parser_name, data_type in sorted(data_type_parsers.items()):
        parser_lines = _data_type_parser_lines(catalogs, data_type)
        if len(parser_lines) == 1 and parser_lines[0].startswith("return "):
            cpp += _single_statement_function(
                f"std::optional<Value> {parser_name}(ValueRange& range)", parser_lines[0]
            ) + "\n\n"
        else:
            cpp += f"std::optional<Value> {parser_name}(ValueRange& range) {{\n"
            cpp += "".join(f"    {line}\n" for line in parser_lines)
            cpp += "}\n\n"

    for property in longhand_parsers:
        if property.syntax is None:
            continue
        call = _parser_call(property, property.syntax, catalogs, "range")
        cpp += _single_statement_function(
            f"std::optional<Value> parse{_pascal_name(property.name)}(ValueRange& range)", f"return {call};"
        ) + "\n\n"

    cpp += (
        "bool parseProperty(Property property, ValueRange& range, "
        "ParsedProperties& result) {\n"
        "    switch (property) {\n"
    )
    for property in property_parsers:
        name = _pascal_name(property.name)
        if property.longhands:
            cpp += (
                f"    case Property::{name}:\n"
                f"        return parseShorthand<Property::{name}>(range, result);\n"
            )
        else:
            cpp += (
                f"    case Property::{name}:\n"
                f"        return parseLonghand<Property::{name}>(range, result);\n"
            )
    cpp += "    default:\n        return false;\n    }\n}\n"
    cpp = cpp.rstrip("\n") + "\n} // namespace Core::CSS\n"
    return header, cpp


def _typed_properties(catalogs: Catalogs) -> list[PropertyDefinition]:
    return [property for property in catalogs.properties if property.type_name and property.storage_path]


def _initial_atom(
    value: str,
    css_keywords: Iterable[str],
    property_name: str,
    *,
    css_namespace: str = "CSS",
) -> str:
    css_prefix = f"{css_namespace}::" if css_namespace else ""
    if value in css_keywords:
        return f"{css_prefix}Keyword::{_pascal_name(value)} {{}}"

    match = INITIAL_VALUE.fullmatch(value)
    if not match:
        raise GenerationError(f"{property_name}: unsupported initial value component {value!r}")

    number, unit = match.groups()
    if "." not in number and "e" not in number.lower():
        number += "."
    number += "f"
    if unit is None:
        return f"{css_prefix}Number({number})"

    unit_name = CSS_INITIAL_UNITS.get(unit.lower())
    if unit_name is None:
        raise GenerationError(f"{property_name}: unsupported initial unit {unit!r}")
    return f"{css_prefix}{unit_name}({number})"


def _initial_value_expression(initial: str, css_keywords: Iterable[str], property_name: str) -> str:
    values = initial.split()
    if not values:
        raise GenerationError(f"{property_name}: initial value is required for generated properties")

    expressions = [_initial_atom(value, css_keywords, property_name) for value in values]
    if len(expressions) == 1:
        return expressions[0]
    return f"{{{', '.join(expressions)}}}"


def _initial_expression(property: PropertyDefinition, css_keywords: Iterable[str]) -> str:
    if property.type_name == "Image" and property.initial == "none":
        return "    return Image {std::monostate {}};"
    if property.type_name == "BorderImageSlice":
        initial = INITIAL_VALUE.fullmatch(property.initial)
        if initial is None or initial.group(2) not in {None, "%"}:
            raise GenerationError(f"{property.name}: BorderImageSlice initial must be a number or percentage")
        return f"    return {_initial_value_expression(property.initial, css_keywords, property.name)};"
    if property.type_name in {"BorderImageWidth", "BorderImageOutset"}:
        initial = INITIAL_VALUE.fullmatch(property.initial)
        if initial is None or initial.group(2) is not None:
            raise GenerationError(f"{property.name}: {property.type_name} initial must be a number")
        return f"    return {_initial_value_expression(property.initial, css_keywords, property.name)};"
    if property.type_name == "BorderImageRepeat":
        if property.initial not in {"stretch", "repeat", "round", "space"}:
            raise GenerationError(f"{property.name}: unsupported BorderImageRepeat initial {property.initial!r}")
        mode = f"BorderImageRepeatMode::{_enum_member_name(property.initial)}"
        return f"    return BorderImageRepeat {{{mode}, {mode}}};"
    if property.type_name == "FlexWrap" and property.initial == "nowrap":
        return "    return {};"
    if property.type_name in {"Length", "OutlineOffset"} and property.initial == "0px":
        return "    return {};"
    if property.type_name == "BoxShadows" and property.initial == "none":
        return "    return {};"
    if property.type_name == "MarginEdge" and property.initial == "0px":
        return "    return {};"
    if property.type_name == "PaddingEdge" and property.initial == "0px":
        return "    return {};"
    if property.type_name == "VerticalAlignValue" and property.initial == "baseline":
        return "    return {};"
    if property.type_name == "Translate" and property.initial == "none":
        return "    return {};"
    if property.type_name == "ColorScheme" and property.initial == "normal":
        return "    return {};"
    if property.type_name in {
        "Dimension", "FlexBasis", "MaximumSize", "MinimumSize", "OptionalDimension", "PreferredSize"
    } and property.initial in {"auto", "none"}:
        return "    return {};"
    if property.type_name == "InsetEdge" and property.initial == "auto":
        return "    return {};"
    if property.type_name in {"LetterSpacing", "WordSpacing"} and property.initial == "normal":
        return "    return {};"
    keyword = _initial_keyword(property)
    if keyword:
        return f"    return CSS::Keyword::{_pascal_name(keyword)} {{}};"
    if property.type_name == "float":
        initial = INITIAL_VALUE.fullmatch(property.initial)
        if initial is None or (initial.group(2) is not None and not (initial.group(2).lower() == "px" and float(initial.group(1)) == 0)):
            raise GenerationError(f"{property.name}: float initial value must be unitless or zero pixels")
        number = initial.group(1)
        if "." not in number and "e" not in number.lower():
            number += ".0"
        return f"    return {number}f;"
    if property.type_name == "Order":
        if not re.fullmatch(r"[+-]?\d+", property.initial):
            raise GenerationError(f"{property.name}: Order initial value must be an integer")
        return f"    return Order {{{int(property.initial)}}};"
    if property.type_name == "ScrollbarGutter" and property.initial == "auto":
        return "    return ScrollbarGutter::Auto;"
    if property.type_name == "Filter" and property.initial == "none":
        return "    return {};"
    if property.type_name == "ScrollbarColor" and property.initial == "auto":
        return "    return {};"
    if property.type_name == "GapGutter" and property.initial == "normal":
        return "    return {};"
    if property.type_name == "BorderStyle":
        if len(property.initial.split()) != 1:
            raise GenerationError(f"{property.name}: BorderStyle initial value must be one keyword")
        return f"    return BorderStyle::{_enum_member_name(property.initial)};"
    if property.type_name == "Color":
        if len(property.initial.split()) != 1 or property.initial not in css_keywords:
            raise GenerationError(f"{property.name}: Color initial value must be one CSS keyword")
        return f"    return Color::fromKeyword(Keyword{_pascal_name(property.initial, reserved=False)});"
    if property.type_name == "FontFamilies":
        return "    return FontFamilies {GenericFontFamily::SansSerif};"
    if property.type_name == "FontFamily":
        return f"    return FontFamily::{_enum_member_name(property.initial)};"
    if property.type_name == "FontStyle":
        return f"    return FontStyle::{_enum_member_name(property.initial)};"
    if property.type_name in {"AlignContent", "JustifyContent"} and property.initial == "normal":
        return "    return {ContentPosition::Normal, ContentDistribution::Default, OverflowAlignment::Default};"
    if property.type_name in {"AlignItems", "JustifyItems"} and property.initial == "normal":
        return "    return {ItemPosition::Normal, OverflowAlignment::Default};"
    if property.type_name == "JustifyItems" and property.initial == "legacy":
        return "    return {ItemPosition::Normal, OverflowAlignment::Default, true};"
    if property.type_name in {"AlignSelf", "JustifySelf"} and property.initial == "auto":
        return "    return {ItemPosition::Auto, OverflowAlignment::Default};"
    if property.type_name == "Appearance" and property.initial == "none":
        return "    return Appearance::NoneValue;"
    if property.storage_kind == "enum":
        if len(property.initial.split()) != 1:
            raise GenerationError(f"{property.name}: enum initial value must be one keyword")
        return f"    return {property.type_name}::{_enum_member_name(property.initial)};"
    return f"    return {_initial_value_expression(property.initial, css_keywords, property.name)};"


def _property_operations(catalogs: Catalogs, keyword_converters: dict[str, tuple[str, ...]]) -> str:
    lines: list[str] = []
    for type_name, keywords in keyword_converters.items():
        lines.extend([*_keyword_enum_converter_lines(type_name, keywords), ""])
    for property in _typed_properties(catalogs):
        name = _pascal_name(property.name, reserved=False)
        member_name = _property_member_name(property)
        lines.extend(
            [
                f"static bool applyValue{name}(BuilderState& builderState, const CSS::StyleValue& value) {{",
                f"    auto parsedValue = toStyle<{property.type_name}>(builderState, value);",
                "    if (!parsedValue)",
                "        return false;",
                f"    builderState.style.set{name}(std::move(*parsedValue));",
                "    return true;",
                "}",
                "",
            ]
        )
        if _initial_keyword(property) and property.type_name in {"float", "FontWeight", "FontWidth"}:
            initial_value = [
                f"    const CSS::Value initialValue {{CSS::Keyword {{ComputedStyle::initial{name}().value}}}};",
                f"    auto parsedValue = toStyle<{property.type_name}>(builderState, initialValue);",
                "    if (!parsedValue)",
                "        return false;",
                f"    builderState.style.set{name}(std::move(*parsedValue));",
            ]
        else:
            initial_value = [
                f"    builderState.style.set{name}(ComputedStyle::initial{name}());",
            ]
        lines.extend(
            [
                f"static bool applyInitial{name}(BuilderState& builderState) {{",
                *initial_value,
                "    return true;",
                "}",
                "",
                f"static bool applyInherit{name}(BuilderState& builderState) {{",
                "    if (!builderState.parentStyle)",
                "        return false;",
                f"    builderState.style.set{name}(builderState.parentStyle->{member_name}());",
                "    return true;",
                "}",
                "",
            ]
        )
    lines.extend(
        [
        "bool applyProperty(CSS::Property property, BuilderState& builderState, ApplyType type, const CSS::StyleValue* specifiedValue) {",
            "    switch (property) {",
        ]
    )
    for property in _typed_properties(catalogs):
        name = _pascal_name(property.name, reserved=False)
        enum_name = _pascal_name(property.name)
        lines.extend(
            [
                f"    case CSS::Property::{enum_name}:",
                "        switch (type) {",
                "        case ApplyType::Value:",
                f"            return specifiedValue && applyValue{name}(builderState, *specifiedValue);",
                "        case ApplyType::Initial:",
                f"            return applyInitial{name}(builderState);",
                "        case ApplyType::Inherit:",
                f"            return applyInherit{name}(builderState);",
                "        }",
                "        return false;",
            ]
        )
    lines.extend(["    default:", "        return false;", "    }", "}", ""])
    shorthands = [property for property in catalogs.properties if property.longhands]
    if shorthands:
        lines.extend(
            [
                "std::optional<ShorthandDescriptor> shorthand(CSS::Property property) {",
                "    switch (property) {",
            ]
        )
        for property in shorthands:
            lines.append(
                f"    case CSS::Property::{_pascal_name(property.name)}:\n"
                f"        return shorthand<CSS::Property::{_pascal_name(property.name)}>();"
            )
        lines.extend(
            [
                "    default:\n        return std::nullopt;",
                "    }",
                "}",
                "",
            ]
        )
    return "\n".join(lines).rstrip("\n")


def generate_html_names(catalogs: Catalogs) -> tuple[str, str]:
    tags = list(catalogs.html_tags)
    attributes = list(catalogs.html_attributes)
    interfaces = sorted({metadata["interface"] for metadata in catalogs.html_tags.values()})
    header = _banner("HTMLTags.json5 and HTMLAttributes.json5") + (
        "#pragma once\n\n"
        "#include <cstddef>\n"
        "#include <cstdint>\n"
        "#include <iterator>\n"
        "#include <optional>\n"
        "#include <string_view>\n\n"
        "namespace Core {\n"
    )
    header += _enum_header("HTMLInterface", interfaces, underlying="std::uint8_t", prefix=("Unknown",)) + "\n\n"
    header += _enum_header("HTMLTag", tags, underlying="std::uint8_t", prefix=("Unknown",))
    header += "\n\nenum class HTMLAttribute : std::uint16_t {\n"
    header += "".join(f"    {_html_attribute_cpp_name(name)},\n" for name in attributes)
    header += "    Count\n};"
    header += (
        "\n\nstruct HTMLTagDescriptor {\n"
        "    HTMLTag tag;\n"
        "    std::string_view name;\n"
        "    HTMLInterface interface;\n"
        "    bool isVoid;\n"
        "};\n"
    )
    header += "\ninline constexpr HTMLTagDescriptor htmlTags[] {\n"
    header += "    {HTMLTag::Unknown, \"\", HTMLInterface::Unknown, false},\n"
    for name in tags:
        metadata = catalogs.html_tags[name]
        is_void = "true" if metadata.get("void", False) else "false"
        header += (
            f"    {{HTMLTag::{_pascal_name(name)}, {_cpp_string(name)}, "
            f"HTMLInterface::{_pascal_name(metadata['interface'])}, {is_void}}},\n"
        )
    header += "};\n\n"
    header += "constexpr const HTMLTagDescriptor* htmlTagDescriptor(HTMLTag tag) {\n"
    header += "    const auto index = static_cast<std::size_t>(tag);\n"
    header += "    return index < std::size(htmlTags) ? &htmlTags[index] : nullptr;\n"
    header += "}\n\n"
    header += "constexpr HTMLInterface HTMLTagInterface(HTMLTag tag) {\n"
    header += (
        "    if (const auto* descriptor = htmlTagDescriptor(tag))\n"
        "        return descriptor->interface;\n"
    )
    header += "    return HTMLInterface::Unknown;\n"
    header += "}\n\n"
    header += "constexpr std::string_view HTMLTagName(HTMLTag tag) {\n"
    header += (
        "    if (const auto* descriptor = htmlTagDescriptor(tag))\n"
        "        return descriptor->name;\n"
    )
    header += "    return {};\n"
    header += "}\n\n"
    header += "constexpr bool isVoidHTMLTag(HTMLTag tag) {\n"
    header += (
        "    if (const auto* descriptor = htmlTagDescriptor(tag))\n"
        "        return descriptor->isVoid;\n"
    )
    header += "    return false;\n"
    header += "}\n\n"
    header += "struct HTMLAttributeDescriptor {\n    HTMLAttribute attribute;\n    std::string_view name;\n};\n"
    header += "\ninline constexpr HTMLAttributeDescriptor htmlAttributes[] {\n"
    for name in attributes:
        header += f"    {{HTMLAttribute::{_html_attribute_cpp_name(name)}, {_cpp_string(name)}}},\n"
    header += "};\n\n"
    header += "constexpr const HTMLAttributeDescriptor* htmlAttributeDescriptor(HTMLAttribute attribute) {\n"
    header += "    const auto index = static_cast<std::size_t>(attribute);\n"
    header += "    return index < std::size(htmlAttributes) ? &htmlAttributes[index] : nullptr;\n"
    header += "}\n\n"
    header += "constexpr std::string_view HTMLAttributeName(HTMLAttribute attribute) {\n"
    header += (
        "    if (const auto* descriptor = htmlAttributeDescriptor(attribute))\n"
        "        return descriptor->name;\n"
    )
    header += "    return {};\n"
    header += "}\n\n"
    header += (
        "HTMLTag findHTMLTag(std::string_view);\n"
        "std::optional<HTMLAttribute> findHTMLAttribute(std::string_view);\n"
        "} // namespace Core\n"
    )
    cpp = _banner("HTMLTags.json5 and HTMLAttributes.json5") + (
        '#include "HTMLNames.h"\n'
        "#include <string>\n"
        '#include "HTMLName.h"\n\n'
        "namespace Core {\n"
    )
    cpp += "HTMLTag findHTMLTag(std::string_view name) {\n"
    cpp += "    const std::string canonical = canonicalizeHTMLName(name);\n"
    cpp += "    for (const auto& descriptor : htmlTags)\n"
    cpp += "        if (descriptor.tag != HTMLTag::Unknown && canonical == descriptor.name)\n            return descriptor.tag;\n"
    cpp += "    return HTMLTag::Unknown;\n"
    cpp += "}\n\n"
    cpp += "std::optional<HTMLAttribute> findHTMLAttribute(std::string_view name) {\n"
    cpp += "    for (const auto& descriptor : htmlAttributes)\n"
    cpp += "        if (descriptor.name == name)\n            return descriptor.attribute;\n"
    cpp += "    return std::nullopt;\n"
    cpp += "}\n} // namespace Core\n"
    return header, cpp


def generate_events(catalogs: Catalogs) -> tuple[str, str]:
    names = list(catalogs.events)
    cpp_names = {name: _event_cpp_name(name) for name in names}
    header = _banner("EventTypes.json5") + (
        "#pragma once\n\n"
        "#include <cstddef>\n"
        "#include <cstdint>\n"
        "#include <iterator>\n"
        "#include <optional>\n"
        "#include <string_view>\n"
        '#include "HTMLNames.h"\n\n'
        "namespace Core {\n"
        "enum class EventType : std::uint16_t {\n"
    )
    header += "".join(f"    {cpp_names[name]},\n" for name in names)
    header += (
        "    Count\n};\n\n"
        "struct EventTypeDescriptor {\n"
        "    EventType type;\n"
        "    std::string_view name;\n"
        "    std::optional<HTMLAttribute> htmlAttribute;\n"
        "};\n\n"
    )
    header += "inline constexpr EventTypeDescriptor eventTypes[] {\n"
    for name in names:
        attribute = "on" + name
        html_attribute = (
            f"HTMLAttribute::{_html_attribute_cpp_name(attribute)}"
            if attribute in catalogs.html_attributes
            else "std::nullopt"
        )
        header += f"    {{EventType::{cpp_names[name]}, {_cpp_string(name)}, {html_attribute}}},\n"
    header += "};\n\n"
    header += "constexpr const EventTypeDescriptor* eventTypeDescriptor(EventType type) {\n"
    header += "    const auto index = static_cast<std::size_t>(type);\n"
    header += "    return index < std::size(eventTypes) ? &eventTypes[index] : nullptr;\n"
    header += "}\n\n"
    header += "constexpr std::string_view eventTypeName(EventType type) {\n"
    header += "    if (const auto* descriptor = eventTypeDescriptor(type))\n        return descriptor->name;\n"
    header += "    return {};\n"
    header += "}\n\n"
    header += "std::optional<EventType> findEventType(std::string_view);\n} // namespace Core\n"
    cpp = _banner("EventTypes.json5") + (
        '#include "EventTypes.h"\n\n'
        "namespace Core {\n"
        "std::optional<EventType> findEventType(std::string_view name) {\n"
        "    for (const auto& descriptor : eventTypes)\n"
        "        if (descriptor.name == name)\n"
        "            return descriptor.type;\n"
        "    return std::nullopt;\n"
        "}\n"
        "} // namespace Core\n"
    )
    return header, cpp


def generate_pseudo_selectors(catalogs: Catalogs) -> tuple[str, str]:
    pseudo_classes = catalogs.pseudo_selectors["pseudo-classes"]
    pseudo_elements = catalogs.pseudo_selectors["pseudo-elements"]
    class_names = list(pseudo_classes)
    element_names = list(pseudo_elements)
    header = _banner("CSSPseudoSelectors.json5") + (
        "#pragma once\n\n"
        "#include <cstddef>\n"
        "#include <cstdint>\n"
        "#include <iterator>\n"
        "#include <optional>\n"
        "#include <string_view>\n\n"
        "namespace Core::CSS {\n"
    )
    header += _enum_header("PseudoClass", class_names, underlying="std::uint8_t", include_count=False)
    header += "\n\nenum class PseudoClassArgumentRequirement : std::uint8_t {\n    None,\n    Optional,\n    Required,\n};\n"
    header += (
        "\nenum class PseudoClassArgumentSyntax : std::uint8_t {\n"
        "    None,\n"
        "    Ident,\n"
        "    CompoundSelector,\n"
        "    ForgivingSelectorList,\n"
        "};\n"
    )
    header += "\nenum class PseudoClassSpecificity : std::uint8_t {\n    Class,\n    Argument,\n    ClassPlusArgument,\n    Zero,\n};\n"
    header += "\nusing ArgumentRequirement = PseudoClassArgumentRequirement;\n"
    header += "using ArgumentSyntax = PseudoClassArgumentSyntax;\n"
    header += "using Specificity = PseudoClassSpecificity;\n"
    header += (
        "\nstruct PseudoClassDescriptor {\n"
        "    PseudoClass pseudoClass;\n"
        "    std::string_view name;\n"
        "    ArgumentRequirement argumentRequirement;\n"
        "    ArgumentSyntax argumentSyntax;\n"
        "    Specificity specificity;\n"
        "};\n"
    )
    header += "\ninline constexpr PseudoClassDescriptor pseudoClasses[] {\n"
    for name in class_names:
        metadata = pseudo_classes[name]
        requirement = {"optional": "Optional", "required": "Required"}.get(metadata.get("argument_requirement"), "None")
        syntax = {
            "ident": "Ident",
            "compound-selector": "CompoundSelector",
            "forgiving-selector-list": "ForgivingSelectorList",
        }.get(metadata.get("argument_syntax"), "None")
        if (requirement == "None") != (syntax == "None"):
            raise GenerationError(f"CSSPseudoSelectors.json5.pseudo-classes.{name}: incomplete argument metadata")
        specificity = {
            "argument": "Argument",
            "class-plus-argument": "ClassPlusArgument",
            "zero": "Zero",
        }.get(metadata.get("specificity"), "Class")
        header += (
            f"    {{PseudoClass::{_pascal_name(name)}, {_cpp_string(name)}, "
            f"ArgumentRequirement::{requirement}, ArgumentSyntax::{syntax}, "
            f"Specificity::{specificity}}},\n"
        )
    header += "};\n\n"
    header += "constexpr const PseudoClassDescriptor* pseudoClassDescriptor(PseudoClass pseudoClass) {\n"
    header += "    const auto index = static_cast<std::size_t>(pseudoClass);\n"
    header += "    return index < std::size(pseudoClasses) ? &pseudoClasses[index] : nullptr;\n"
    header += "}\n\n"
    header += "std::optional<PseudoClass> findPseudoClass(std::string_view);\n\n"
    header += "constexpr std::string_view pseudoClassName(PseudoClass pseudoClass) {\n"
    header += "    if (const auto* descriptor = pseudoClassDescriptor(pseudoClass))\n        return descriptor->name;\n"
    header += "    return {};\n"
    header += "}\n\n"

    header += _enum_header("PseudoElement", element_names, underlying="std::uint8_t", include_count=False)
    header += (
        "\n\nstruct PseudoElementDescriptor {\n"
        "    PseudoElement pseudoElement;\n"
        "    std::string_view name;\n"
        "    bool userAgent;\n"
        "};\n"
    )
    header += "\ninline constexpr PseudoElementDescriptor pseudoElements[] {\n"
    for name in element_names:
        metadata = pseudo_elements[name]
        user_agent = "true" if metadata.get("user_agent", False) else "false"
        header += f"    {{PseudoElement::{_pascal_name(name)}, {_cpp_string(name)}, {user_agent}}},\n"
    header += "};\n\n"
    header += "constexpr const PseudoElementDescriptor* pseudoElementDescriptor(PseudoElement pseudoElement) {\n"
    header += "    const auto index = static_cast<std::size_t>(pseudoElement);\n"
    header += "    return index < std::size(pseudoElements) ? &pseudoElements[index] : nullptr;\n"
    header += "}\n\n"
    header += "std::optional<PseudoElement> findPseudoElement(std::string_view);\n\n"
    header += "constexpr std::string_view pseudoElementName(PseudoElement pseudoElement) {\n"
    header += "    if (const auto* descriptor = pseudoElementDescriptor(pseudoElement))\n        return descriptor->name;\n"
    header += "    return {};\n"
    header += "}\n\n"
    header += "constexpr bool isUserAgentPseudoElement(PseudoElement pseudoElement) {\n"
    header += "    if (const auto* descriptor = pseudoElementDescriptor(pseudoElement))\n        return descriptor->userAgent;\n"
    header += "    return false;\n"
    header += "}\n} // namespace Core::CSS\n"

    cpp = _banner("CSSPseudoSelectors.json5") + (
        '#include "CSSPseudoSelectors.h"\n\n'
        "namespace Core::CSS {\n"
    )
    cpp += "std::optional<PseudoClass> findPseudoClass(std::string_view name) {\n"
    cpp += "    for (const auto& descriptor : pseudoClasses)\n"
    cpp += "        if (descriptor.name == name)\n            return descriptor.pseudoClass;\n"
    cpp += "    return std::nullopt;\n"
    cpp += "}\n\n"
    cpp += "std::optional<PseudoElement> findPseudoElement(std::string_view name) {\n"
    cpp += "    for (const auto& descriptor : pseudoElements)\n"
    cpp += "        if (descriptor.name == name)\n            return descriptor.pseudoElement;\n"
    cpp += "    return std::nullopt;\n"
    cpp += "}\n} // namespace Core::CSS\n"
    return header, cpp


def generate_input_types(catalogs: Catalogs) -> tuple[str, str]:
    names = list(catalogs.input_types)
    header = _banner("InputTypes.json5") + (
        "#pragma once\n\n"
        "#include <cstddef>\n"
        "#include <cstdint>\n"
        "#include <iterator>\n"
        "#include <optional>\n"
        "#include <string_view>\n\n"
        "namespace Core {\n"
    )
    header += _enum_header("InputType", names)
    header += "\n\nstruct InputTypeDescriptor {\n    InputType type;\n    std::string_view name;\n};\n"
    header += "\ninline constexpr InputTypeDescriptor inputTypes[] {\n"
    for name in names:
        header += f"    {{InputType::{_pascal_name(name)}, {_cpp_string(name)}}},\n"
    header += "};\n\n"
    header += "constexpr const InputTypeDescriptor* inputTypeDescriptor(InputType type) {\n"
    header += "    const auto index = static_cast<std::size_t>(type);\n"
    header += "    return index < std::size(inputTypes) ? &inputTypes[index] : nullptr;\n"
    header += "}\n\n"
    header += "constexpr std::string_view inputTypeName(InputType type) {\n"
    header += "    if (const auto* descriptor = inputTypeDescriptor(type))\n        return descriptor->name;\n"
    header += "    return {};\n"
    header += "}\n\nstd::optional<InputType> findInputType(std::string_view);\n} // namespace Core\n"
    cpp = _banner("InputTypes.json5") + (
        '#include "InputTypes.h"\n\n'
        "namespace Core {\n"
        "std::optional<InputType> findInputType(std::string_view name) {\n"
    )
    cpp += "    for (const auto& descriptor : inputTypes)\n"
    cpp += "        if (descriptor.name == name)\n            return descriptor.type;\n"
    cpp += "    return std::nullopt;\n"
    cpp += "}\n} // namespace Core\n"
    return header, cpp


def generate_outputs(catalogs: Catalogs) -> dict[str, str]:
    property_header, property_cpp = generate_property_names(catalogs)
    keyword_header, keyword_cpp = generate_keyword_names(catalogs)
    computed_header, computed_cpp, computed_inlines = generate_computed_style(catalogs)
    parsing_header, parsing_cpp = generate_property_parsing(catalogs)
    html_header, html_cpp = generate_html_names(catalogs)
    event_header, event_cpp = generate_events(catalogs)
    pseudo_header, pseudo_cpp = generate_pseudo_selectors(catalogs)
    input_header, input_cpp = generate_input_types(catalogs)
    outputs = {
        "CSSProperties.h": property_header,
        "CSSProperties.cpp": property_cpp,
        "CSSKeywords.h": keyword_header,
        "CSSKeywords.cpp": keyword_cpp,
        "CSSPropertyParsing.h": parsing_header,
        "CSSPropertyParsing.cpp": parsing_cpp,
        "ComputedStyleProperties.h": computed_header,
        "ComputedStyleProperties.cpp": computed_cpp,
        "ComputedStylePropertiesInlines.h": computed_inlines,
        "HTMLNames.h": html_header,
        "HTMLNames.cpp": html_cpp,
        "EventTypes.h": event_header,
        "EventTypes.cpp": event_cpp,
        "CSSPseudoSelectors.h": pseudo_header,
        "CSSPseudoSelectors.cpp": pseudo_cpp,
        "InputTypes.h": input_header,
        "InputTypes.cpp": input_cpp,
    }
    return outputs


def generate(source_root: Path, output_root: Path) -> list[Path]:
    catalogs = load_catalogs(source_root)
    outputs = generate_outputs(catalogs)
    output_root.mkdir(parents=True, exist_ok=True)
    paths: list[Path] = []
    for name, content in outputs.items():
        path = output_root / name
        with path.open("w", encoding="utf-8", newline="\n") as stream:
            stream.write(content)
        paths.append(path)
    return paths


def generate_user_agent_stylesheet(source_path: Path, output_root: Path) -> Path:
    try:
        stylesheet = source_path.read_text(encoding="utf-8")
    except OSError as error:
        raise GenerationError(f"cannot read stylesheet {source_path}: {error}") from error

    if ')__RADIA__"' in stylesheet:
        raise GenerationError(f"{source_path}: contains the reserved raw-string terminator")

    output_root.mkdir(parents=True, exist_ok=True)
    output_path = output_root / "UserAgentStyleSheet.cpp"
    source = (
        "// Generated automatically from ua.css, do not edit.\n\n"
        "#include \"UserAgentStyleSheet.h\"\n\n"
        "namespace Core::CSS {\n"
        "std::string_view userAgentStyleSheet() noexcept {\n"
        '    return R"__RADIA__(\n'
        + stylesheet
        + ')__RADIA__";\n'
        "}\n"
        "} // namespace Core::CSS\n"
    )
    with output_path.open("w", encoding="utf-8", newline="\n") as stream:
        stream.write(source)
    return output_path


def _check_output(source_root: Path, output_root: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="radia-codegen-") as directory:
        generated = generate(source_root, Path(directory))
        expected = {path.name: path.read_text(encoding="utf-8") for path in generated}
        actual = {path.name: path.read_text(encoding="utf-8") for path in output_root.glob("*") if path.is_file()}
        if expected != actual:
            missing = sorted(set(expected) - set(actual))
            extra = sorted(set(actual) - set(expected))
            changed = sorted(name for name in set(expected) & set(actual) if expected[name] != actual[name])
            details = []
            if missing:
                details.append(f"missing {missing}")
            if extra:
                details.append(f"extra {extra}")
            if changed:
                details.append(f"changed {changed}")
            raise GenerationError("generated output is not deterministic: " + "; ".join(details))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Generate Radia UI catalog C++ files")
    default_root = Path(__file__).resolve().parents[1]
    parser.add_argument("--source-root", type=Path, default=default_root)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--user-agent-stylesheet", type=Path)
    parser.add_argument("--check", action="store_true", help="compare existing output with a fresh generation")
    args = parser.parse_args(argv)
    try:
        if args.user_agent_stylesheet:
            if args.check:
                parser.error("--check cannot be used with --user-agent-stylesheet")
            print(generate_user_agent_stylesheet(args.user_agent_stylesheet, args.output))
        elif args.check:
            _check_output(args.source_root.resolve(), args.output.resolve())
        else:
            paths = generate(args.source_root.resolve(), args.output.resolve())
            for path in paths:
                print(path)
    except GenerationError as error:
        print(f"codegen: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
