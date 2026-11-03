// Review probes for Spike 4, added to a scratch copy only.

fn probe(md_path: &str, assets: &str, out_stem: &str) {
    let markdown = std::fs::read_to_string(md_path).unwrap();
    println!("=== {md_path}");
    for (name, mechanism) in [("table", Mechanism::Table), ("literal", Mechanism::Literal)] {
        let emitted = emit(&markdown, &Page::default(), mechanism);
        if emitted.source.len() < 900 {
            println!("--- {name} source:\n{}", emitted.source);
        }
        let compiled = compile(&EMBEDDED, emitted.source, Some(PathBuf::from(assets)));
        match compiled.document {
            Ok(document) => {
                let laid = laid_out(&document);
                let shown: String = laid.text.chars().take(160).collect();
                println!("--- {name}: {} pages, {} missing glyphs, warnings {:?}, emitter errors {:?}\n    laid out: {shown:?}", document.pages().len(), laid.missing, compiled.warnings, emitted.errors);
                std::fs::write(format!("{out_stem}-{name}.pdf"), to_pdf(&document).unwrap()).unwrap();
            }
            Err(e) => println!("--- {name}: COMPILE ERROR {}", e.chars().take(200).collect::<String>()),
        }
    }
}

// Review probes for the fixes proposed for Stage 7 and 8.

/// Absolute boxes for text on one page. Ignores rotation and scale, which the Renderer never emits.
fn text_boxes(frame: &Frame, offset: typst::layout::Point, out: &mut Vec<(f64, f64, f64, String)>) {
    for (pos, item) in frame.items() {
        let at = offset + *pos;
        match item {
            FrameItem::Group(group) => {
                let moved = at + typst::layout::Point::new(group.transform.tx, group.transform.ty);
                text_boxes(&group.frame, moved, out);
            }
            FrameItem::Text(text) => out.push((at.y.to_pt(), at.x.to_pt(), (at.x + text.width()).to_pt(), text.text.chars().take(24).collect())),
            _ => {}
        }
    }
}

fn layout_report(document: &PagedDocument) -> (usize, usize, Vec<String>) {
    let (mut past_edge, mut overlaps, mut samples) = (0, 0, Vec::new());
    for (number, page) in document.pages().iter().enumerate() {
        let width = page.frame.width().to_pt();
        let mut boxes = Vec::new();
        text_boxes(&page.frame, typst::layout::Point::zero(), &mut boxes);
        for (_, _, right, text) in &boxes {
            if *right > width {
                past_edge += 1;
                if samples.len() < 6 {
                    samples.push(format!("page {}: past the edge at {right:.0} of {width:.0}: {text:?}", number + 1));
                }
            }
        }
        boxes.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        for pair in boxes.windows(2) {
            let same_line = (pair[0].0 - pair[1].0).abs() < 0.1;
            if same_line && pair[1].1 + 0.5 < pair[0].2 {
                overlaps += 1;
                if samples.len() < 6 {
                    samples.push(format!("page {}: {:?} overlaps {:?}", number + 1, pair[0].3, pair[1].3));
                }
            }
        }
    }
    (past_edge, overlaps, samples)
}

fn layout_check(paths: &[String], assets: &str) {
    for path in paths {
        let markdown = std::fs::read_to_string(path).unwrap();
        let emitted = emit(&markdown, &Page::default(), Mechanism::Literal);
        let compiled = compile(&EMBEDDED, emitted.source, Some(PathBuf::from(assets)));
        match compiled.document {
            Ok(document) => {
                let (past, overlaps, samples) = layout_report(&document);
                println!("{}: {} pages, {past} past the page edge, {overlaps} overlapping on a line", Path::new(path).file_name().unwrap().to_str().unwrap(), document.pages().len());
                for sample in samples {
                    println!("    {sample}");
                }
            }
            Err(e) => println!("{path}: COMPILE ERROR {e}"),
        }
    }
}

/// The header note with and without repeating, and a note on a row instead.
fn header_note(path: &str, out_stem: &str) {
    let markdown = std::fs::read_to_string(path).unwrap();
    for (name, replace) in [("repeating", false), ("repeat false", true)] {
        let mut source = emit(&markdown, &Page::default(), Mechanism::Literal).source;
        if replace {
            source = source.replace("table.header(", "table.header(repeat: false, ");
        }
        let document = compile(&EMBEDDED, source, None).document.unwrap();
        let pdf = format!("{out_stem}-{}.pdf", name.replace(' ', "-"));
        std::fs::write(&pdf, to_pdf(&document).unwrap()).unwrap();
        let text = pdftotext(Path::new(&pdf));
        println!("{name}: {} pages, the note appears {} times", document.pages().len(), text.matches("The note on a repeated header").count());
    }
}
