# ADR-0011: Restore dropped XLSX parts from the input

**Date:** 2026-11-02
**Status:** accepted
**Deciders:** Andreas Heissenberger
**Related:** `docs/adr/0004-initial-crate-set.md` · `docs/adr/0010-scrub-the-written-xlsx-package.md` · `docs/specs/0010-xlsx-round-trip-fidelity.md` · `docs/specs/0012-restore-xlsx-external-links.md` · `docs/specs/0013-restore-xlsx-sparklines.md`

## Context

Spec 0010 ran pseudofy on an Excel-made workbook. `umya-spreadsheet` 3.1.1 drops two features from the copy:

- External links. The copy keeps each formula that uses a link, for example `[1]!ContactsTable[...]`. The link part is missing, so the formula points to nothing. Excel shows a repair message for this copy.
- Sparklines. The copy has no sparkline groups in the `extLst` element of the sheet.

The scrub of ADR-0010 already reads and writes each output package with `zip` and `quick-xml`.
The input package has the dropped XML. Neither feature stores a pseudonymized cell value in a sheet.

## Decision

We will restore the dropped XML from the input package in the scrub of ADR-0010.

- The scrub gets the input package and the output package.
- The scrub copies only XML that a spec names. It copies no other part from the input.
- The scrub removes each cached cell value from restored XML before it writes the output.
- The scrub finds the sheet of the output by the sheet name, not by the part name.
- If the scrub cannot restore a feature, pseudofy fails for that file. It writes no copy with a broken reference.

Spec 0012 restores external links. Spec 0013 restores sparklines.

## Consequences

### Positive

- Formulas that use an external link stay valid in the copy.
- Sparklines stay in the copy.
- No new crate enters `Cargo.lock`.
- `umya-spreadsheet` stays. pseudofy needs no XLSX writer of its own.

### Negative / Accepted costs

- The scrub depends on more of the ECMA-376 structure: relationships, content types, the order of elements in `workbook.xml`, and namespace declarations.
- An external link opens in Excel without cached values. Excel shows the values after the user updates the link.
- The restored target path of an external link can contain a user name, for example `/Users/<name>/`. pseudofy does not change paths (README, limits).
- Each new dropped feature needs a new spec.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Replace `umya-spreadsheet` with own XML editing of all parts | High effort. Shared strings, rich text, and styles are error-prone (ADR-0004). |
| Refuse workbooks with external links or sparklines | Users cannot process common workbooks. |
| Accept the loss and document it | The copy has broken formulas. Excel can show a repair message. This breaks requirement 9. |
| Copy restored parts with their cached values | Cached values of a linked workbook can contain real names. |

## Compliance & verification (optional)

`tests/xlsx_fidelity.rs` lists no external link part as lost.
A test checks that the copy has the same number of sparklines as the input.
A test checks that no restored part has a cached cell value.
