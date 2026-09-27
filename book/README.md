# PWE book (HonKit)

A GitBook-compatible book built from the repository's own Markdown sources —
no content is copied: [`SUMMARY.md`](../SUMMARY.md) at the repo root links the
canonical pages (`README.md`, `docs/`, `rfc/`, `std/README.md`).

## Commands

```sh
cd book
npm install          # honkit + the local pwe-highlight plugin
npm run build        # → book/_book/ (gitignored)
npm run serve        # http://localhost:4000
```

The book root is the repository root (`honkit build .. book/_book`), so the
toolchain stays in `book/` while the sources stay where they are.

## Layout

| Path | Role |
| --- | --- |
| `../SUMMARY.md` | Table of contents (parts + every RFC). |
| `../book.json` | Title, links, plugins. |
| `../.bookignore` | Whitelist: only README/docs/rfc/std/tasks sources are copied. |
| `plugins/pwe-highlight/` | Registers the `pwe` language so PWE code blocks highlight. |
| `../docs/book/` | Chapters written for the book (quick start, examples, determinism, architecture, FAQ). |
| `_book/` | Build output — ignored by git. |

## Notes

* PWE code fences are highlighted by `plugins/pwe-highlight`, a ~60-line
  highlight.js language definition; without it HonKit logs
  `Unknown language: "pwe"` and falls back to plain text.
* Pages must live inside the book root — HonKit rejects `../` paths in
  `SUMMARY.md`, which is why the root is the repository root rather than
  `book/`.
