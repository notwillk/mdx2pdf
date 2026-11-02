# mdx2pdf

A self-contained Rust command-line tool that turns a Markdown file, or a directory of MDX with frontmatter, Liquid templating, and reusable components, into a PDF. Everything is first reduced to plain Markdown, and that Markdown is what gets typeset.

**Status: design phase.** There's no code yet. The current work is proving the riskiest assumptions with [spikes](docs/spikes.md).

## Docs

- [Product requirements](docs/prd.md): what the tool does and doesn't do
- [Technical design](docs/technical-design.md): the pipeline, packages, and delivery order
- [Spikes](docs/spikes.md): the experiments that come before implementation
- [Decisions](docs/adr/): choices that are hard to reverse
- [Glossary](GLOSSARY.md): the words these docs use
