#set page(paper: "a4", margin: 25mm)
#set text(font: "Libertinus Serif", size: 11pt)
#set document(title: "Report Title", date: none)
#heading(level: 1)[Report Title]

#heading(level: 2)[Second level]

#heading(level: 3)[Third level]

A paragraph with #emph[emphasis];\, #strong[strong];\, #strike[struck];\, #raw("inline code");\, and a #link("https://example.com/a?b=c")[link];\. A soft break\, and then a hard break\: after the break\, with a backslash break\:#linebreak();after that\.

#list(tight: true, [bullet one], [bullet two#list(tight: true, [nested bullet#list(tight: true, [doubly nested])

])

], [bullet three])

#enum(start: 1, tight: false, [ordered one

], [ordered two

#enum(start: 1, tight: true, [nested ordered])

], [started at seven

], [and eight

])

#list(tight: false, [loose one

], [loose two

], [#"☑ ";done task

], [#"☐ ";todo task

#list(tight: true, [#"☑ ";nested task])

])

#table(columns: 3, align: (left, center, right,), table.header([Left], [Center], [Right]), [a1], [b1], [c1], [a2], [b2 \| pipe], [c2], [short], [], [],)

#raw(block: true, lang: "rust", "fn main() { println!(\"{}\", \"code # $ @ block\"); }")

#raw(block: true, lang: none, "no language\n    indented * line")

#quote(block: true)[A quote with #strong[strong]; text\.

#quote(block: true)[A nested quote\.

]

#list(tight: true, [a list in a quote])

]

#list(tight: false, [A list holding a quote\:

#quote(block: true)[quoted in a list

]

])

A footnote here#[#footnote[The first note\.

]<fn-0>];\, another#[#footnote[The second note\, with two paragraphs\.

Its second paragraph\, with #strong[strong];\.

]<fn-1>];\, and the first again#footnote(<fn-0>);\.

#line(length: 100%)

#box(image("photo.png", alt: "Photo alt"));

#box(image("logo.svg", alt: "Logo alt"));

Trailing paragraph\.

