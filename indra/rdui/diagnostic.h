/**
 * @file diagnostic.h
 * @brief
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

#ifndef RD_DIAGNOSTIC_H
#define RD_DIAGNOSTIC_H

#include <cstddef>
#include <iterator>
#include <string>
#include <utility>
#include <vector>

namespace rdui {
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
} // namespace rdui
#endif // RD_DIAGNOSTIC_H
