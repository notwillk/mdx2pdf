# mdx2pdf

The language of a tool that reduces templated documents to Markdown and then to PDF.

## Language

### Inputs

**Markdown Input**:
A single `.md` file that is rendered exactly as written, with no frontmatter, Liquid, or Components.
_Avoid_: plain input, raw file

**Project**:
A directory containing an Entry Document, with optional Components and Page Settings.
_Avoid_: workspace, bundle, site

**Entry Document**:
The `index.mdx` file at the root of a Project.
_Avoid_: root file, main file

**Component**:
An MDX file in a Project that a tag such as `<Callout />` refers to by name.
_Avoid_: template, partial, macro

**Page Settings**:
Document-wide paper and type settings, such as paper size and margins.
_Avoid_: theme, config, layout

### The format

**Dialect**:
The variant of MDX this tool reads: MDX tags and Expressions, YAML frontmatter, and Liquid, with no JavaScript.
_Avoid_: "MDX" on its own when the difference from Standard MDX matters

**Standard MDX**:
The language defined by the MDX project, in which expressions are JavaScript and files can `import` and `export`.
_Avoid_: real MDX

**Meta**:
The frontmatter values of the file being evaluated.
_Avoid_: variables, page data

**Props**:
The named values a Component declares in its frontmatter and receives from the tag that uses it.
_Avoid_: arguments, parameters

**Content Slot**:
The `<Content />` tag, which marks where a Component places the children its caller passed in.
_Avoid_: children placeholder, `{props.children}`

**Liquid Pass**:
The evaluation of `{{ }}` and `{% %}` over a file's text before that text is read as MDX. Its result can create tags.
_Avoid_: preprocessing, templating step

**Expression**:
A single-brace `{...}` holding a Liquid value and filters. Its result is data and can never create markup.
_Avoid_: interpolation, JSX expression

### Outputs

**Expansion**:
The replacement of Component tags, Content Slots, and Expressions until only Markdown remains.
_Avoid_: compiling, rendering

**Intermediate Markdown**:
The Markdown a Renderer reads: the result of Expansion for a Project, or the file itself for a Markdown Input.
_Avoid_: compiled Markdown, output Markdown

**Renderer**:
The part of the tool that turns Intermediate Markdown and Page Settings into a PDF.
_Avoid_: backend, exporter

## Relationships

- A **Project** has exactly one **Entry Document**, any number of **Components**, and at most one set of **Page Settings**
- A **Component** may use other **Components**, but never itself, directly or indirectly
- A **Component** has its own **Meta** and **Props**; the children placed in its **Content Slot** keep the scope of the file that wrote them
- Any piece of text goes through the **Liquid Pass** at most once
- A **Markdown Input** has no **Meta**, **Liquid Pass**, or **Expansion**
- A **Renderer** reads only **Intermediate Markdown** and **Page Settings**, so it can't tell which kind of input produced them
