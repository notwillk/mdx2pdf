---
status: accepted
---

# Liquid is the only logic language

Documents use MDX tags, but all logic and value lookups use Liquid syntax, and there's no JavaScript, `import`, or `export`. `{{ }}` and `{% %}` run over each file's text before it's read as MDX, so they can generate tags. `{ }` runs after that and inserts data that can't become markup. We chose this to keep one logic language in a self-contained binary, where documents can't execute code. The cost is that this is a dialect: Standard MDX files and most MDX tooling won't work unchanged, and a literal `{{` needs `{% raw %}`, even inside a code fence.

## Considered options

- **Embedded JavaScript:** closest to Standard MDX and to executable.md, but adds binary size, sandboxing work, and a second logic language next to Liquid.
- **Data lookups only in `{ }`:** simpler to implement, but gives filters only to `{{ }}`, so the safe form would be the weaker one.
- **Liquid in frontmatter values only:** avoids escaping problems, but Liquid couldn't loop or branch over body content.
- **No Liquid:** one syntax and a simpler pipeline, but it would need built-in `<If>` and `<Loop>` components, and Liquid was part of the brief.
