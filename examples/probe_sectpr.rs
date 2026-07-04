use agentic_export::book::*;
use std::io::Read;

fn main() {
    let chapters: Vec<(String, String)> = (1..=14).map(|i| (
        format!("ch{}", i),
        format!("# Chapter {} title\n\nBody prose for chapter {}.\n\nA second paragraph.\n", i, i)
    )).collect();
    let meta = BookMeta { title: "T".into(), thesis_profile: true, emit_per_chapter_sectpr: true, ..Default::default() };
    let bytes = render_book(&meta, &chapters, std::path::Path::new(".")).unwrap();
    std::fs::write("C:/Users/dcaso/AppData/Local/Temp/probe.docx", &bytes).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut xml = String::new();
    zip.by_name("word/document.xml").unwrap().read_to_string(&mut xml).unwrap();
    let count = xml.matches("<w:sectPr").count();
    println!("sectprs={}", count);
    // print first body sectpr context
    if let Some(idx) = xml.find("<w:sectPr") {
        let end = (idx + 400).min(xml.len());
        println!("first sectpr: {}", &xml[idx..end]);
    }
}
