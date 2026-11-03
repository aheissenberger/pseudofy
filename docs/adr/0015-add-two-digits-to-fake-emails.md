# ADR-0015: Add two digits to fake emails

**Date:** 2026-11-03
**Status:** accepted
**Deciders:** Andreas Heissenberger
**Related:** `docs/adr/0003-derive-pseudonyms-with-a-keyed-hash.md` · `docs/specs/0022-fewer-pseudonym-collisions.md`

## Context

The local part of a fake email is a fake first name and a fake last name: `sophie.wagner`. The German lists give about 3.1 million local parts.
A project with 10 000 different emails in one domain gets about 16 collisions: two real emails get one fake email.
The email is often the column that tells persons apart in an analysis of the output.
Requirement 7 says that one real value gets the same pseudonym in each later run. A change of the email format changes each fake email.

## Decision

We will add two digits from 10 to 99 to the local part of each fake email, and accept the change of each fake email in release 0.3.0.

- The HMAC output of the email selects the digits (ADR-0003). The digits do not depend on the order of files or rows.
- The format is `first.last` plus the digits: `sophie.wagner42@label.example.com`.
- The fake names do not change. `tests/pseudonym_stability.rs` gets new golden emails only.

## Consequences

### Positive

- Collisions of fake emails drop by a factor of 90: about 16 to about 0.2 for 10 000 emails.
- Numbers in an email local part are common. The fake emails still look real.

### Negative / Accepted costs

- Each fake email of a project from 0.2.0 changes. The README tells the user in the release notes.
- Collisions do not drop to zero. pseudofy still reports each collision that it detects.

## Alternatives considered

| Alternative | Why rejected |
| --- | --- |
| Three digits | Fewer collisions, but the local part looks less natural. Two digits already give about 0.2 collisions for 10 000 emails. |
| A saved table of pseudonyms | ADR-0003 rejects a mapping table. The table needs the same protection as the input. |
| A retry with the next name on a collision | The result depends on the order of files and rows. One real email can get two fake emails in two runs. |
| Larger name lists | Each pseudonym changes, also the fake names. New lists need a clear license. |

## Compliance & verification (optional)

`tests/pseudonym_stability.rs` checks the new golden emails and the unchanged fake names. Unit tests in `src/pseudonym.rs` check the digits and the collision count.
