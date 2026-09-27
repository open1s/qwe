# 0029 — Mailbox blockquote wedged mid-table breaks the system-parameter table (Low)

## Summary

Commit `c7ef2a65` (the #7 channel-docs fix) inserted the new mailbox note
as a **blockquote between two rows of a Markdown table**
(`docs/lang-usage.md:603-610`):

```markdown
| `send`/`recv` | `on`(req),`chan`,`value` / … | channel | channel send/receive (scoped to `on`) |

> **Channels are a single-cell mailbox, not a Go-style queue.** …
> step's value; place it after for same-step delivery.
| `update` | `dt`,`on?`,`when?`,… | state | explicit Euler |
| `rk4` | … | … | … |
| `invariant` | … | … | … |
```

GFM terminates a table at block-level content, so every row **after** the
blockquote (`update`, `rk4`, `invariant`, `nbody`, …) no longer renders as
table cells.

## Evidence

Rendered the fragment through GitHub's own Markdown API
(`POST /markdown`, 2026-09-27):

```
</table>
<blockquote>
<p><strong>Channels are a single-cell mailbox.</strong> …</p>
</blockquote>
<p>| update | dt |
| rk4 | dt |</p>
```

The first table closes before the blockquote and the following rows come
back as a single paragraph of literal pipe text. Same breakage applies to
the HonKit book build and any GFM viewer. Note this also hides the
`on`(req) documentation introduced by `8b892c1c` in the same row block,
since the reader loses the table context.

## Impact

The `docs/lang-usage.md` system-parameter reference — the main lookup
table for system kinds and their parameters — renders as raw text from
`update` downward on GitHub and in the book. Introduced by the #7 fix,
so the fix for one docs issue broke the doc it lives in.

## Suggested fix

Move the mailbox note **below the entire table** (after the last row and
blank line), or fold it into the `send`/`recv` row's last cell / a table
footnote. Keep all `| … |` rows contiguous.
