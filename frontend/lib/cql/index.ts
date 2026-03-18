/**
 * CQL (CXDB Query Language) - Frontend Module
 *
 * A JQL-like query language for searching and filtering CXDB contexts.
 *
 * Example queries:
 *   tag = "amplifier"
 *   tag = "amplifier" AND user = "jay"
 *   (service = "dotrunner" OR service = "gen") AND created > "-7d"
 *   service ^= "dot"
 *   user ~= "Jay"
 *   tag IN ("amplifier", "dotrunner", "gen")
 *   NOT tag = "test"
 */

export { Lexer, tokenize } from './lexer';
export { Parser, parse, validate } from './parser';
export {
  // Token types
  type Token,
  type TokenType,
  type Position,

  // AST types
  type Expression,
  type AndExpr,
  type OrExpr,
  type NotExpr,
  type Comparison,
  type Operator,
  type Value,
  type StringValue,
  type NumberValue,
  type DateValue,
  type ListValue,
  type CqlQuery,

  // Error types
  type CqlError,
  type CqlResult,

  // Field metadata
  type FieldName,
  type FieldMeta,
  VALID_FIELDS,
  FIELD_METADATA,
} from './types';

/**
 * Serialize a CQL AST to JSON for backend transmission
 */
export function serializeQuery(query: import('./types').CqlQuery): string {
  return JSON.stringify({
    raw: query.raw,
    ast: query.ast,
  });
}

/**
 * Format a CQL error for display
 */
export function formatError(error: import('./types').CqlError): string {
  return `${error.message} (line ${error.position.line}, column ${error.position.column})`;
}

const TRAILING_TAG_CLAUSE_PATTERN = /\s+AND\s+tag\s*=\s*"((?:\\"|[^"])*)"$/i;
const EXACT_TAG_CLAUSE_PATTERN = /^tag\s*=\s*"((?:\\"|[^"])*)"$/i;

/**
 * Split the visible search text into its base query and any appended trailing tag clause.
 */
export function extractTagSearchClause(query: string): { baseQuery: string; tag: string | null } {
  const trimmedQuery = query.trim();
  const exactMatch = trimmedQuery.match(EXACT_TAG_CLAUSE_PATTERN);
  if (exactMatch) {
    return {
      baseQuery: '',
      tag: exactMatch[1].replace(/\\"/g, '"'),
    };
  }

  const trailingMatch = trimmedQuery.match(TRAILING_TAG_CLAUSE_PATTERN);
  if (!trailingMatch) {
    return { baseQuery: trimmedQuery, tag: null };
  }

  return {
    baseQuery: trimmedQuery.slice(0, trailingMatch.index).trim(),
    tag: trailingMatch[1].replace(/\\"/g, '"'),
  };
}

/**
 * Replace any existing trailing tag clause with the clicked context tag.
 */
export function appendTagSearchClause(query: string, tag: string): string {
  const normalizedTag = tag.trim();
  const escapedTag = normalizedTag.replace(/"/g, '\\"');
  const { baseQuery } = extractTagSearchClause(query);

  if (!normalizedTag) {
    return baseQuery;
  }

  return baseQuery
    ? `${baseQuery} AND tag = "${escapedTag}"`
    : `tag = "${escapedTag}"`;
}

/**
 * Build a fallback query that searches across all text fields.
 * Used when input doesn't parse as valid CQL - treats it as a keyword search.
 *
 * @param term - The search term to look for
 * @returns A CQL query string that searches all relevant fields
 */
export function buildFallbackQuery(term: string): string {
  // Escape quotes in the term
  const escaped = term.replace(/"/g, '\\"');

  // Check if term looks like a numeric ID
  const isNumeric = /^\d+$/.test(term.trim());

  // Fields to search with case-insensitive prefix match (^~=)
  // These fields support the starts_ci operator
  const prefixFields = ['tag', 'title', 'user', 'service', 'host'];

  // Build OR clauses for string fields using case-insensitive prefix match
  const clauses = prefixFields.map(f => `${f} ^~= "${escaped}"`);

  // Add exact match for id if numeric
  if (isNumeric) {
    clauses.unshift(`id = ${term.trim()}`);
  }

  // Note: 'label' only supports eq/neq/in, so we use exact match
  // This is less useful for fuzzy search but won't error
  clauses.push(`label = "${escaped}"`);

  return clauses.join(' OR ');
}
