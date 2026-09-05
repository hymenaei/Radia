/**
 * Copyright (C) 2026 Radia Viewer
 * SPDX-License-Identifier: LGPL-2.1-only
 */

#pragma once

#include <cstddef>
#include <iterator>
#include <string>
#include <utility>
#include <vector>

namespace radia::ui {
enum class DiagnosticSeverity { Warning, Error };

struct Diagnostic {
    DiagnosticSeverity severity;
    std::string code;
    std::string message;
    std::string source;
    std::size_t line = 0;
    std::size_t column = 0;

    std::string formatted() const {
        if (source.empty()) return message;
        std::string location = source;
        if (line) {
            location += ":" + std::to_string(line);
            if (column) location += ":" + std::to_string(column);
        }
        return location + ": " + message;
    }
};

struct DiagnosticResult {
    std::vector<Diagnostic> warnings;
    std::vector<Diagnostic> errors;

    bool hasErrors() const { return !errors.empty(); }

    void append(DiagnosticResult&& source) {
        warnings.insert(warnings.end(), std::make_move_iterator(source.warnings.begin()), std::make_move_iterator(source.warnings.end()));
        errors.insert(errors.end(), std::make_move_iterator(source.errors.begin()), std::make_move_iterator(source.errors.end()));
    }

    void warning(std::string code, std::string message, std::string source = {}, std::size_t line = 0, std::size_t column = 0) {
        warnings.push_back({DiagnosticSeverity::Warning, std::move(code), std::move(message), std::move(source), line, column});
    }

    void error(std::string code, std::string message, std::string source = {}, std::size_t line = 0, std::size_t column = 0) {
        errors.push_back({DiagnosticSeverity::Error, std::move(code), std::move(message), std::move(source), line, column});
    }
};
} // namespace radia::ui
