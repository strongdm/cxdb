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

export interface SearchCriterionClause {
  field: 'tag' | 'label';
  value: string;
}

const TRAILING_CRITERION_PATTERN = /\s+AND\s+(tag|label)\s*=\s*"((?:\\"|[^"])*)"$/i;
const EXACT_CRITERION_PATTERN = /^(tag|label)\s*=\s*"((?:\\"|[^"])*)"$/i;

/**
 * Split the visible search text into its base query and any appended trailing search criteria.
 */
export function extractSearchCriteriaClauses(query: string): {
  baseQuery: string;
  criteria: SearchCriterionClause[];
} {
  const trimmedQuery = query.trim();
  const exactMatch = trimmedQuery.match(EXACT_CRITERION_PATTERN);
  if (exactMatch) {
    return {
      baseQuery: '',
      criteria: [{
        field: exactMatch[1].toLowerCase() as SearchCriterionClause['field'],
        value: exactMatch[2].replace(/\\"/g, '"'),
      }],
    };
  }

  const criteria: SearchCriterionClause[] = [];
  let baseQuery = trimmedQuery;

  while (true) {
    const trailingMatch = baseQuery.match(TRAILING_CRITERION_PATTERN);
    if (!trailingMatch || trailingMatch.index === undefined) {
      break;
    }
    criteria.unshift({
      field: trailingMatch[1].toLowerCase() as SearchCriterionClause['field'],
      value: trailingMatch[2].replace(/\\"/g, '"'),
    });
    baseQuery = baseQuery.slice(0, trailingMatch.index).trim();
  }

  return {
    baseQuery,
    criteria,
  };
}

/**
 * Append a clickable search facet to the visible query.
 *
 * `tag` criteria replace any prior appended `tag` criterion because contexts only
 * have one client tag. `label` criteria accumulate so users can narrow by multiple
 * labels from the sidebar.
 */
export function appendSearchCriterionClause(
  query: string,
  criterion: SearchCriterionClause
): string {
  const normalizedValue = criterion.value.trim();
  const escapedValue = normalizedValue.replace(/"/g, '\\"');
  const { baseQuery, criteria } = extractSearchCriteriaClauses(query);

  if (!normalizedValue) {
    return baseQuery;
  }

  const nextCriteria =
    criterion.field === 'tag'
      ? [...criteria.filter(existing => existing.field !== 'tag'), { field: 'tag', value: normalizedValue }]
      : criteria.some(existing => existing.field === 'label' && existing.value === normalizedValue)
        ? criteria
        : [...criteria, { field: 'label', value: normalizedValue }];

  const renderedCriteria = nextCriteria.map(
    existing => `${existing.field} = "${existing.value.replace(/"/g, '\\"')}"`
  );

  return [baseQuery, ...renderedCriteria].filter(Boolean).join(' AND ');
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
