---
status: accepted
---

# Components only ever produce Markdown

Expansion produces only Markdown. Components can't emit raw HTML, CSS, or Typst, and this is a permanent boundary rather than a v1 limit, so please don't add a passthrough as a convenience. Look and feel is global, coming from the Renderer and Page Settings. This keeps every document independent of the typesetting engine, so the engine stays replaceable. The cost is that Components can't produce per-component layout, such as colored callout boxes or columns.

## Considered options

- **Raw Typst blocks in the Intermediate Markdown:** the best typesetting control, but ties documents to Typst.
- **Raw HTML and CSS:** familiar and technically valid Markdown, but needs an HTML-based engine. That means either the young Fulgur or a browser, and the self-contained binary requirement rules out a browser.

## Consequences

Lowercase tags such as `<div>` are errors in the Dialect rather than HTML. A Markdown Input can still contain raw HTML, since it's rendered as written, but the Renderer can't draw it. The default behavior for that case is an open question in the [PRD](../prd.md).
