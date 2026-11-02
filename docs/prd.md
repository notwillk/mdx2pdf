# Product requirements

**Status:** Draft, 2026-11-02. Terms are defined in the [glossary](../GLOSSARY.md). How this gets built is in the [technical design](technical-design.md).

## Problem

Getting a well-typeset PDF from Markdown usually means installing a browser, pandoc, or a LaTeX distribution. Markdown also has no way to reuse content or fill in data, so repeated documents end up copied and edited by hand.

`mdx2pdf` is one binary that takes a document written in a small MDX dialect and produces a PDF, with nothing else to install.

**Assumed users, to confirm:** developers and technical writers producing repeatable documents, such as reports, from a directory of text files, on their own machines or in CI.

## Goals

- Produce a good-looking PDF from Markdown with no setup.
- Let authors reuse content through Components and fill in data through frontmatter and Liquid.
- Reduce every document to plain Markdown before typesetting, and let authors inspect that Markdown.
- Run as a single self-contained binary that works offline.

## Non-goals

These are out of scope. The first two are permanent decisions, and the rest could be revisited.

| Out of scope | Why |
|---|---|
| Raw HTML, CSS, or Typst from Components | Output is only ever Markdown ([ADR 0002](adr/0002-components-only-produce-markdown.md)) |
| JavaScript, `import`, `export` | Liquid is the only logic language ([ADR 0001](adr/0001-liquid-is-the-only-logic-language.md)) |
| Compatibility with Standard MDX files and tooling | Follows from the above: this is a dialect |
| Per-component styling, such as colored boxes or columns | Look and feel is global |
| Running code in documents | Unlike executable.md, this tool only produces a document |
| Remote images | Must work offline and produce repeatable output |
| Math | Not in v1. `$...$` stays literal text |
| Output formats other than PDF, and watch mode | Not needed to prove the tool's value |

## Inputs

| Input | Treatment |
|---|---|
| A `.md` file (**Markdown Input**) | Rendered exactly as written. No frontmatter handling, Liquid, or Components |
| A directory containing `index.mdx` (**Project**) | Frontmatter, Liquid, Expressions, and Components, reduced to Markdown and then rendered |

A leading `---` block in a Markdown Input isn't treated as frontmatter, so it prints as document content.

A Project looks like this:

```text
report/
├── index.mdx          required: the Entry Document
├── page.toml          optional: Page Settings
├── components/        optional: Components
│   └── Callout.mdx
└── chart.png          optional: images and other assets
```

## The format

`components/Callout.mdx` declares its Props in frontmatter and places its children with `<Content />`:

```mdx
---
props:
  type: object
  properties:
    kind: { type: string, default: Note }
---
> **{props.kind | upcase}**
>
> <Content />
```

`index.mdx` uses it, with its data in frontmatter:

```mdx
---
title: Q3 Report
items:
  - name: Alpha
  - name: Beta
---
# {meta.title}

{% for item in items %}
<Callout kind="{{ item.name }}">
Review **{{ item.name }}** before {meta.title} ships.
</Callout>
{% endfor %}
```

The result is this Intermediate Markdown, which is then typeset:

```md
# Q3 Report

> **ALPHA**
>
> Review **Alpha** before Q3 Report ships.

> **BETA**
>
> Review **Beta** before Q3 Report ships.
```

The real text can differ in spacing, such as a ` > ` prefix on block quote lines, but it's the same document.

Authors have two kinds of template syntax, each with its own job:

| | `{{ x }}` and `{% ... %}` | `{ x }` |
|---|---|---|
| **Use it for** | Loops, conditions, generating tags | Printing data, passing Props |
| **Runs** | Before the text is read as MDX | After the text is read as MDX |
| **Its result** | Becomes part of the source, so it can create tags | Stays data, so it can't create markup |

So `{company}` is safe when `company` is `A<B`, while `{{ company }}` isn't. Loop variables exist only during the first kind, so loops use `{{ item.name }}`.

Inside a table cell, escape a filter's pipe with a backslash, as in `{x \| upcase}`, because a bare `|` ends the cell.

## Command line

```text
mdx2pdf <INPUT> [-o <FILE>] [--page <FILE>] [--emit-md <FILE>]
```

| Argument | Meaning |
|---|---|
| `<INPUT>` | A `.md` file or a Project directory |
| `-o <FILE>` | PDF path. Defaults to the input's name with a `.pdf` extension, in the current directory |
| `--page <FILE>` | Page Settings file. For a Project, this replaces `page.toml` |
| `--emit-md <FILE>` | Also write the Intermediate Markdown to this path, or to stdout with `-` |

Exit codes are `0` for success, `1` for a problem with the document, and `2` for incorrect usage.

## Requirements

### Input

- **R1.** Accept a `.md` file as a Markdown Input.
- **R2.** Accept a directory containing `index.mdx` as a Project.
- **R3.** Reject anything else with a message saying what was expected.
- **R4.** Render a Markdown Input exactly as written, with no frontmatter, Liquid, or Component handling.

### Dialect

- **R5.** Read YAML frontmatter from each MDX file, available as `meta.x` and as top-level `x`. `meta` and `props` are reserved names.
- **R6.** Run the Liquid Pass over the whole body of each MDX file before reading it as MDX. Literal `{{` and `{%` need `{% raw %}`, including inside code fences.
- **R7.** Evaluate Expressions using Liquid syntax, such as `{props.name | upcase}`, after reading the file as MDX. Their results can't create markup, and whitespace at the edges of a value is trimmed. In a table cell, a filter's pipe is written `\|`.
- **R8.** Report `import`, `export`, and lowercase tags such as `<div>` as errors that explain the alternative.

### Components

- **R9.** Resolve `<Name />` to `./components/Name.mdx`, then `./Name.mdx`, relative to the Project root. An unknown name is an error.
- **R10.** Validate Props against the Component's frontmatter, and apply declared defaults.
- **R11.** Place a Component's children at `<Content />`, keeping the scope of the file that wrote them.
- **R12.** Allow Components to use other Components, and report recursion as an error that shows the chain.
- **R13.** Expansion produces only Markdown. No form of raw HTML, CSS, or Typst passthrough.

### Markdown and PDF

- **R14.** Intermediate Markdown is CommonMark plus GFM tables, task lists, strikethrough, and footnotes, with no MDX left in it.
- **R15.** `--emit-md` writes the Intermediate Markdown.
- **R16.** The PDF's content comes only from the Intermediate Markdown, Page Settings, and local images.
- **R17.** Support local images, confined to the Project directory, or to the file's directory for a Markdown Input.

### Page Settings

- **R18.** Read optional Page Settings from `page.toml` in a Project, or from `--page`.
- **R19.** Support paper size, orientation, margins, base font and size, and page numbers, with sensible defaults. Values never contain Typst or other engine syntax.
- **R20.** Report unknown keys and invalid values as errors.

### Operation

- **R21.** Ship as one self-contained binary, with no Chrome, pandoc, or LaTeX needed at runtime.
- **R22.** Never use the network.
- **R23.** The same input and tool version produce a byte-identical PDF.
- **R24.** Errors name the file and, where possible, the location and a suggested fix.
- **R25.** If the result of Expansion can't be represented exactly in Markdown, report an error rather than produce a changed PDF.
- **R26.** Report a tag with no closing `>`, and a table that a bare pipe turned into a paragraph, as errors rather than printing them as text.

## Success criteria

1. The example above produces the same document as the Intermediate Markdown shown, and a correct PDF. Differences in spacing don't count, as long as both parse to the same document.
2. The binary produces a PDF on a clean machine with no network and nothing else installed.
3. Two runs on the same input produce byte-identical PDFs.
4. Writing `--emit-md` into the Project directory and rendering that file with the same Page Settings produces a PDF identical to the original.
5. Every hazard listed in the [technical design](technical-design.md#known-hazards) either works correctly or gives an error that explains the fix.

## Open questions

Each has a working default, used until someone decides otherwise.

| Question | Default |
|---|---|
| Raw HTML in a Markdown Input, which is valid Markdown but which the Renderer can't draw | Skip it with a warning |
| A bare `.mdx` file as input | Reject it, with a hint to put it in a directory as `index.mdx` |
| Can Components read the Entry Document's frontmatter? | No, pass values through Props |
| `{/* comment */}`, which MDX authors expect | Accept it as a comment |
| Namespaced Components such as `<Ns.Sub />` | Later |
| Bare URLs such as `https://a.b`, which GFM can turn into links | Print as plain text. Literal autolinks are off in both the Dialect and Markdown, so authors write `[text](url)` for a link |
| Fonts beyond the bundled ones | Load them from a `fonts/` directory in the Project. Never system fonts, because they'd break R23 |
| Performance and binary-size targets | Spike 4 measures them, then a person sets the bar |
