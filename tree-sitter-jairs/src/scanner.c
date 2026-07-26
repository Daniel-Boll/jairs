/**
 * External scanner for Jairs nested block comments.
 *
 * Tree-sitter's built-in regex engine cannot express balanced nesting, so
 * nested /* ... * / comments require a hand-written C scanner.
 *
 * The scanner is called when tree-sitter sees a position where block_comment
 * could appear (i.e., everywhere, since it's in extras). We skip leading
 * whitespace and then check for `/*`.
 *
 * Protocol:
 *   - valid_symbols[0] corresponds to `block_comment` in grammar.js.
 *   - We skip whitespace, then consume `/*`, track depth, and return true
 *     when depth reaches 0.
 *   - On EOF before depth reaches 0 we still return true (consuming what we
 *     have) so the rest of the parse can continue; the compiler's own lexer
 *     will report the diagnostic.
 */

#include "tree_sitter/parser.h"
#include <stdbool.h>
#include <stdint.h>

/* Token indices must match the order in grammar.js `externals`. */
enum TokenType {
    BLOCK_COMMENT,
};

/* No persistent state needed — the scanner is stateless. */
void *tree_sitter_jairs_external_scanner_create(void) { return NULL; }
void  tree_sitter_jairs_external_scanner_destroy(void *payload) { (void)payload; }
void  tree_sitter_jairs_external_scanner_reset(void *payload) { (void)payload; }

unsigned tree_sitter_jairs_external_scanner_serialize(void *payload, char *buffer) {
    (void)payload; (void)buffer;
    return 0;
}

void tree_sitter_jairs_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
    (void)payload; (void)buffer; (void)length;
}

static bool is_whitespace(int32_t c) {
    return c == ' ' || c == '\t' || c == '\n' || c == '\r';
}

bool tree_sitter_jairs_external_scanner_scan(
    void *payload,
    TSLexer *lexer,
    const bool *valid_symbols
) {
    (void)payload;

    if (!valid_symbols[BLOCK_COMMENT]) {
        return false;
    }

    /* Skip whitespace — the internal lexer handles whitespace as trivia, but
     * since block_comment is in extras, we may be called before whitespace is
     * consumed. We skip it here so we can check for `/*`. We do NOT mark_end
     * here; if we don't find `/*` we return false and the internal lexer will
     * handle the whitespace. */
    while (is_whitespace(lexer->lookahead)) {
        lexer->advance(lexer, true); /* skip = true: treat as whitespace */
    }

    /* We must see `/*` to start a block comment. */
    if (lexer->lookahead != '/') {
        return false;
    }
    lexer->advance(lexer, false);

    if (lexer->lookahead != '*') {
        return false;
    }
    lexer->advance(lexer, false);

    /* Now inside the comment at depth 1. */
    int depth = 1;
    int32_t prev = 0;

    while (depth > 0) {
        if (lexer->eof(lexer)) {
            /* Unterminated comment: consume what we have and let the compiler
             * report the diagnostic. Mark the end here. */
            lexer->mark_end(lexer);
            lexer->result_symbol = BLOCK_COMMENT;
            return true;
        }

        int32_t c = lexer->lookahead;
        lexer->advance(lexer, false);

        if (prev == '/' && c == '*') {
            depth++;
            prev = 0; /* reset so `/**` doesn't double-count */
        } else if (prev == '*' && c == '/') {
            depth--;
            prev = 0;
        } else {
            prev = c;
        }
    }

    lexer->mark_end(lexer);
    lexer->result_symbol = BLOCK_COMMENT;
    return true;
}
